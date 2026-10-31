//! Explicit HTTPS mailbox transport. The relay never receives content keys.
pub mod couple;
pub mod http;
pub mod quic;
pub mod relay;
pub mod server;
pub mod setup;
pub mod transport;

use crate::diagnostic::Diagnostic;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub relays: Vec<String>,
    pub state_dir: PathBuf,
    pub admission_token_file: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert_file: Option<PathBuf>,
    #[serde(default = "poll_wait")]
    pub poll_wait_seconds: u64,
    /// Offer only HTTP/1.1. By default TLS (ALPN) negotiates HTTP/2 and falls back to
    /// HTTP/1.1 when the relay or a TLS-inspecting proxy does not offer it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub http1_only: bool,
    /// Experimental: try HTTP/3 over QUIC (UDP) first and fall back to TCP at the first
    /// failure. Not combinable with `proxy_url`: QUIC cannot pass an HTTP proxy.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub http3: bool,
    /// Send writes as POST instead of PUT, for web hosts or firewalls that refuse PUT. The
    /// client also switches by itself when a PUT is answered with 403, 405 or 501.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub post_writes: bool,
}
/// A relay may be mounted below a path, e.g. `https://example.org/wp-json/k-lani-link`.
fn relay_path(path: &str) -> bool {
    let path = path
        .strip_suffix('/')
        .filter(|p| !p.is_empty())
        .unwrap_or(path);
    path == "/"
        || path.len() <= 128
            && path.starts_with('/')
            && path.split('/').skip(1).all(|segment| {
                !segment.is_empty()
                    && segment != "."
                    && segment != ".."
                    && segment
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
            })
}
fn poll_wait() -> u64 {
    LONG_POLL
}
impl Options {
    pub fn resolve(&mut self, base: &Path) {
        for path in [&mut self.state_dir, &mut self.admission_token_file] {
            if path.is_relative() {
                *path = base.join(&*path);
            }
        }
        if let Some(path) = &mut self.ca_cert_file
            && path.is_relative()
        {
            *path = base.join(&*path);
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.relays.len() != 1 {
            return Err(Diagnostic::err(
                "HTTPS transport requires exactly one relay; a second is not enabled",
            ));
        }
        let url = url::Url::parse(&self.relays[0])
            .map_err(|_| Diagnostic::err("invalid HTTPS relay URL"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !relay_path(url.path())
        {
            return Err(Diagnostic::err(
                "HTTPS relay must be an HTTPS URL without credentials, query or fragment; a path may only name where the relay is mounted",
            ));
        }
        if !self.state_dir.is_absolute()
            || !self.admission_token_file.is_absolute()
            || self.ca_cert_file.as_ref().is_some_and(|p| !p.is_absolute())
            || self.poll_wait_seconds > LONG_POLL
        {
            return Err(Diagnostic::err(
                "invalid HTTPS state/token/certificate path or polling wait",
            ));
        }
        if self.http3 && self.proxy_url.is_some() {
            return Err(Diagnostic::err(
                "HTTP/3 cannot pass an HTTPS proxy; disable http3 or proxy_url",
            ));
        }
        if let Some(proxy) = &self.proxy_url {
            let url = url::Url::parse(proxy)
                .map_err(|_| Diagnostic::err("invalid explicit HTTPS proxy URL"))?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(Diagnostic::err(
                    "explicit proxy must be an HTTP or HTTPS origin",
                ));
            }
        }
        Ok(())
    }
}
pub fn public_file(path: &Path, maximum: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(maximum as u64 + 1).read_to_end(&mut bytes))
        .map_err(|_| Diagnostic::err("HTTPS certificate file cannot be read"))?;
    if bytes.len() > maximum {
        return Err(Diagnostic::err("HTTPS certificate file is too large"));
    }
    Ok(bytes)
}
pub fn secret(path: &Path) -> anyhow::Result<String> {
    let info = std::fs::symlink_metadata(path)
        .map_err(|_| Diagnostic::err("HTTPS admission token file cannot be read"))?;
    if !info.is_file() || info.len() > 4096 {
        return Err(Diagnostic::err(
            "HTTPS admission token must be a regular file of at most 4 KiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if info.permissions().mode() & 0o077 != 0 {
            return Err(Diagnostic::err(
                "HTTPS admission token file must be private (chmod 600)",
            ));
        }
    }
    let mut value = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(4097).read_to_string(&mut value))
        .map_err(|_| Diagnostic::err("HTTPS admission token must be readable ASCII text"))?;
    let value = value.trim().to_owned();
    if !(32..=256).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Diagnostic::err(
            "HTTPS admission token must have 32..256 URL-safe ASCII characters",
        ));
    }
    Ok(value)
}

pub const WINDOW: usize = 32;
pub const PIPELINE: usize = 8;
pub const RETRY_HASHES: usize = 64;
pub const RECORD_TTL: u64 = 30;
pub const SESSION_TTL: u64 = 120;
pub const LONG_POLL: u64 = 25;
pub const PEER_TIMEOUT: u64 = 23;
pub const HEARTBEAT: u64 = 10;
pub const BATCH_MS: u64 = 30;
