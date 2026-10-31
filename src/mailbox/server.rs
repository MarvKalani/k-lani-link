//! Native TLS or an explicitly selected private listener behind a TLS proxy.
use super::{
    relay::{Access, Admit, Brake, Limits, Relay},
    secret,
};
use crate::diagnostic::Diagnostic;
use anyhow::Result;
use axum_server::accept::Accept;
use serde::Deserialize;
use std::{
    future::{Ready, ready},
    io::{self, Read},
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::{OwnedSemaphorePermit, Semaphore},
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub token_file: PathBuf,
    /// Tokens naming the same space share rooms and limits: one token per device.
    #[serde(default)]
    pub space: Option<String>,
    /// Last valid day, `YYYY-MM-DD` (UTC); refused from the following midnight.
    #[serde(default)]
    pub expires: Option<String>,
    /// Admitted networks, e.g. `203.0.113.0/24` or `2001:db8::/48`; empty admits any.
    #[serde(default)]
    pub allow_ips: Vec<String>,
    /// Distinct addresses (IPv6 per /64) within 24 hours; 0 means unlimited.
    #[serde(default)]
    pub max_ips_per_day: usize,
    /// Default limits if omitted; a further token of a space takes the space's limits.
    #[serde(default)]
    pub limits: Option<Limits>,
}
impl Admission {
    fn access(&self) -> Result<Access> {
        let expires = match &self.expires {
            Some(day) => Some(day_end(day).ok_or_else(|| {
                Diagnostic::err("relay token `expires` must be a date YYYY-MM-DD")
            })?),
            None => None,
        };
        let allow_ips = self
            .allow_ips
            .iter()
            .map(|text| {
                text.parse::<ipnet::IpNet>()
                    .or_else(|_| text.parse::<std::net::IpAddr>().map(ipnet::IpNet::from))
                    .map(|net| net.trunc())
                    .map_err(|_| {
                        Diagnostic::err("relay token `allow_ips` needs addresses or networks")
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        if allow_ips.len() > 256 || self.max_ips_per_day > 1024 {
            return Err(Diagnostic::err(
                "relay token allows at most 256 networks and 1024 addresses per day",
            ));
        }
        if self.space.as_ref().is_some_and(|name| {
            name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }) {
            return Err(Diagnostic::err(
                "relay token `space` needs 1..64 letters, digits, - or _",
            ));
        }
        Ok(Access {
            expires,
            allow_ips,
            max_ips_per_day: self.max_ips_per_day,
        })
    }
}
/// Unix seconds of the midnight (UTC) after `YYYY-MM-DD`.
fn day_end(text: &str) -> Option<u64> {
    let mut parts = text.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let (year, month, day): (i64, u32, u32) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let length = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if !(1970..=9999).contains(&year) || !(1..=length).contains(&day) {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from((days + 1) * 86_400).ok()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub listen: SocketAddr,
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
    #[serde(default)]
    pub behind_tls_proxy: bool,
    #[serde(default = "connections")]
    pub max_connections: usize,
    /// Offer only HTTP/1.1 over TLS. By default ALPN offers HTTP/2 first; clients and
    /// proxies that do not support it fall back to HTTP/1.1 on the same port.
    #[serde(default)]
    pub http1_only: bool,
    /// Experimental: also serve HTTP/3 over QUIC on the same address via UDP. Needs TLS
    /// in the relay itself; endpoints opt in with `http3` and fall back to TCP.
    #[serde(default)]
    pub http3: bool,
    /// Refused requests per minute and address before that address gets `429` for
    /// `failed_auth_ban_minutes`. 0 (default) turns the brake off. Needs the relay to see
    /// client addresses itself, so not with `behind_tls_proxy`.
    #[serde(default)]
    pub failed_auth_per_minute: u32,
    #[serde(default = "ban_minutes")]
    pub failed_auth_ban_minutes: u64,
    #[serde(default)]
    pub global: Limits,
    pub tokens: Vec<Admission>,
}
fn connections() -> usize {
    256
}
fn ban_minutes() -> u64 {
    10
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(65537).read_to_string(&mut text))
            .map_err(|_| Diagnostic::err("HTTPS relay configuration cannot be read"))?;
        if text.len() > 65536 {
            return Err(Diagnostic::err("HTTPS relay configuration is too large"));
        }
        // The file holds settings and paths, never secrets, so the parser's reason may be shown.
        let mut config: Self = toml::from_str(&text).map_err(|error| {
            let line = error
                .span()
                .map_or(0, |span| text[..span.start].matches('\n').count() + 1);
            Diagnostic::err(format!(
                "invalid HTTPS relay configuration (line {line}): {}",
                error.message()
            ))
        })?;
        let absolute = std::fs::canonicalize(path)
            .map_err(|_| Diagnostic::err("HTTPS relay configuration cannot be located"))?;
        let base = absolute
            .parent()
            .ok_or_else(|| Diagnostic::err("invalid HTTPS relay configuration path"))?;
        for p in [&mut config.tls_cert, &mut config.tls_key]
            .into_iter()
            .flatten()
        {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
        for admission in &mut config.tokens {
            if admission.token_file.is_relative() {
                admission.token_file = base.join(&admission.token_file);
            }
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.tls_cert.is_some() != self.tls_key.is_some()
            || (!self.listen.ip().is_loopback()
                && self.tls_cert.is_none()
                && !self.behind_tls_proxy)
            || !(1..=8192).contains(&self.max_connections)
        {
            return Err(Diagnostic::err(
                "relay needs matching TLS files, a loopback/private-proxy listener and bounded connections",
            ));
        }
        self.global.validate()?;
        if self.tokens.is_empty() || self.tokens.len() > 64 {
            return Err(Diagnostic::err(
                "relay requires 1..64 admission token files",
            ));
        }
        for admission in &self.tokens {
            if let Some(limits) = &admission.limits {
                limits.validate()?;
            }
            if self.behind_tls_proxy && admission.access()?.needs_peer() {
                return Err(Diagnostic::err(
                    "allow_ips and max_ips_per_day need client addresses, not behind_tls_proxy",
                ));
            }
        }
        if self.failed_auth_per_minute > 10_000
            || !(1..=1440).contains(&self.failed_auth_ban_minutes)
            || (self.behind_tls_proxy && self.failed_auth_per_minute > 0)
        {
            return Err(Diagnostic::err(
                "failed_auth_per_minute needs 0..10000, ban 1..1440 minutes and no behind_tls_proxy",
            ));
        }
        Ok(())
    }
    pub fn relay(&self) -> Result<Arc<Relay>> {
        self.validate()?;
        let tokens = self
            .tokens
            .iter()
            .map(|t| {
                Ok(Admit {
                    key: secret(&t.token_file)?,
                    space: t.space.clone(),
                    limits: t.limits.clone(),
                    access: t.access()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Relay::with_access(
            self.global.clone(),
            tokens,
            Brake {
                failures_per_minute: self.failed_auth_per_minute,
                ban: Duration::from_secs(self.failed_auth_ban_minutes * 60),
            },
        )
    }
    pub async fn tls(&self) -> Result<Option<axum_server::tls_rustls::RustlsConfig>> {
        let (Some(cert), Some(key)) = (&self.tls_cert, &self.tls_key) else {
            return Ok(None);
        };
        let info = std::fs::symlink_metadata(key)
            .map_err(|_| Diagnostic::err("relay TLS key cannot be read"))?;
        if !info.is_file() || info.len() > 128 * 1024 {
            return Err(Diagnostic::err(
                "relay TLS key must be a bounded regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if info.permissions().mode() & 0o077 != 0 {
                return Err(Diagnostic::err("relay TLS key must be private (chmod 600)"));
            }
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        axum_server::tls_rustls::RustlsConfig::from_pem(
            super::public_file(cert, 128 * 1024)?,
            super::public_file(key, 128 * 1024)?,
        )
        .await
        .map(Some)
        .map_err(|_| Diagnostic::err("invalid or unreadable relay TLS certificate/key"))
    }
}

#[derive(Clone)]
struct Limited {
    slots: Arc<Semaphore>,
}
struct Stream {
    socket: TcpStream,
    _slot: OwnedSemaphorePermit,
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_read(cx, buf)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().socket).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().socket).poll_shutdown(cx)
    }
}
impl<S> Accept<TcpStream, S> for Limited {
    type Stream = Stream;
    type Service = S;
    type Future = Ready<io::Result<(Stream, S)>>;
    fn accept(&self, socket: TcpStream, service: S) -> Self::Future {
        let result = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| io::Error::other("relay connection capacity reached"))
            .and_then(|slot| {
                socket.set_nodelay(true)?;
                Ok((
                    Stream {
                        socket,
                        _slot: slot,
                    },
                    service,
                ))
            });
        ready(result)
    }
}
pub async fn serve(
    listener: TcpListener,
    relay: Arc<Relay>,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
    maximum: usize,
    http1_only: bool,
    stop: CancellationToken,
) -> Result<()> {
    let router = relay.router(stop.clone());
    serve_router(listener, router, tls, maximum, http1_only, stop).await
}
/// Bounds for HTTP/2 connections. Each request still passes the relay's own admission
/// limits (waiting GETs, pending PUTs, rate), which apply per stream, not per connection.
const H2_STREAMS: u32 = 64;
const H2_HEADER_BYTES: u32 = 16 * 1024;
pub(crate) async fn serve_router(
    listener: TcpListener,
    router: axum::Router,
    tls: Option<axum_server::tls_rustls::RustlsConfig>,
    maximum: usize,
    http1_only: bool,
    stop: CancellationToken,
) -> Result<()> {
    if !(1..=8192).contains(&maximum) {
        return Err(Diagnostic::err("invalid relay connection limit"));
    }
    let handle = axum_server::Handle::new();
    let close = handle.clone();
    let closing = stop.clone();
    let shutdown = tokio::spawn(async move {
        closing.cancelled().await;
        close.graceful_shutdown(Some(Duration::from_secs(2)));
    });
    let acceptor = Limited {
        slots: Arc::new(Semaphore::new(maximum)),
    };
    let socket = listener.into_std()?;
    let result = if let Some(tls) = tls {
        let mut inner = (*tls.get_inner()).clone();
        inner.alpn_protocols = if http1_only {
            vec![b"http/1.1".to_vec()]
        } else {
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        };
        let tls = axum_server::tls_rustls::RustlsConfig::from_config(Arc::new(inner));
        let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(tls)
            .handshake_timeout(Duration::from_secs(5))
            .acceptor(acceptor);
        let mut server = axum_server::from_tcp(socket)?
            .acceptor(acceptor)
            .handle(handle);
        if http1_only {
            server = server.http1_only();
        }
        let builder = server.http_builder();
        builder
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(Duration::from_secs(10));
        builder
            .http2()
            .timer(hyper_util::rt::TokioTimer::new())
            .max_concurrent_streams(H2_STREAMS)
            .max_header_list_size(H2_HEADER_BYTES)
            // Bound what an unread upload can park: one record per stream.
            .initial_stream_window_size(32 * 1024)
            .initial_connection_window_size(1024 * 1024)
            .keep_alive_interval(Duration::from_secs(20))
            .keep_alive_timeout(Duration::from_secs(10));
        server
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
    } else {
        let mut server = axum_server::from_tcp(socket)?
            .acceptor(acceptor)
            .handle(handle)
            .http1_only();
        server
            .http_builder()
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(Duration::from_secs(10));
        server
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
    };
    stop.cancel();
    shutdown.abort();
    result.map_err(anyhow::Error::from)
}
pub async fn run(settings: Settings, stop: CancellationToken) -> Result<()> {
    let relay = settings.relay()?;
    let tls = settings.tls().await?;
    let listener = TcpListener::bind(settings.listen).await?;
    if settings.http3 {
        let Some(tls) = &tls else {
            return Err(Diagnostic::err(
                "HTTP/3 needs tls_cert and tls_key in the relay itself",
            ));
        };
        let endpoint = super::quic::listen(settings.listen, &tls.get_inner())
            .map_err(|_| Diagnostic::err("relay HTTP/3 (UDP) listener could not start"))?;
        tokio::spawn(super::quic::serve(
            endpoint,
            relay.router(stop.clone()),
            settings.max_connections,
            stop.clone(),
        ));
    }
    serve(
        listener,
        relay,
        tls,
        settings.max_connections,
        settings.http1_only,
        stop,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(extra: &str, token: &str) -> Result<Settings> {
        let settings: Settings = toml::from_str(&format!(
            "listen = \"127.0.0.1:1\"\n{extra}\n[[tokens]]\ntoken_file = \"t\"\n{token}"
        ))?;
        settings.validate()?;
        settings.tokens[0].access().map(|_| settings)
    }
    #[test]
    fn expiry_dates_end_at_the_following_utc_midnight() {
        assert_eq!(day_end("1970-01-01"), Some(86_400));
        assert_eq!(day_end("2000-02-29"), Some(951_868_800));
        assert_eq!(day_end("2026-12-31"), Some(1_798_761_600));
        for bad in [
            "2026-02-29",
            "2026-13-01",
            "2026-1-01",
            "26-01-01",
            "2026-01-01x",
            "",
        ] {
            assert_eq!(day_end(bad), None, "{bad}");
        }
    }
    #[test]
    fn access_options_are_checked_when_loaded() {
        assert!(settings("", "expires = \"2027-01-31\"\nallow_ips = [\"192.0.2.0/24\", \"2001:db8::1\"]\nmax_ips_per_day = 8\nspace = \"me\"").is_ok());
        assert!(settings("", "expires = \"31.01.2027\"").is_err());
        assert!(settings("", "allow_ips = [\"192.0.2.0/33\"]").is_err());
        assert!(settings("", "space = \"my space\"").is_err());
        assert!(settings("", "max_ips_per_day = 5000").is_err());
        assert!(settings("failed_auth_per_minute = 20", "").is_ok());
        assert!(
            settings(
                "failed_auth_per_minute = 20\nfailed_auth_ban_minutes = 0",
                ""
            )
            .is_err()
        );
    }
    #[test]
    fn address_options_are_refused_behind_a_tls_proxy() {
        assert!(settings("behind_tls_proxy = true", "").is_ok());
        assert!(settings("behind_tls_proxy = true", "allow_ips = [\"192.0.2.0/24\"]").is_err());
        assert!(settings("behind_tls_proxy = true", "max_ips_per_day = 8").is_err());
        assert!(settings("behind_tls_proxy = true\nfailed_auth_per_minute = 20", "").is_err());
    }
}
