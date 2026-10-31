//! Finite verified HTTPS requests. Retries retain the original sealed bytes.
use super::{BATCH_MS, Options, PIPELINE, WINDOW, secret};
use crate::diagnostic::Diagnostic;
use anyhow::Result;
use axum::body::Bytes;
use k_lani_link_protocol::MAX_PACKET_BYTES;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Network,
    Gone,
    Denied,
    Invalid,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Network => "HTTPS transport interrupted",
            Self::Gone => "HTTPS session records are no longer available",
            Self::Denied => "HTTPS relay rejected access or protocol",
            Self::Invalid => "invalid HTTPS relay response",
        })
    }
}
impl std::error::Error for Failure {}
/// A relay's bounded Retry-After hint, carried as context on a retryable failure.
#[derive(Debug, Clone, Copy)]
struct RetryAfter(Duration);
impl std::fmt::Display for RetryAfter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTPS relay asked to retry later")
    }
}
/// A failed request that may still have reached the relay: no connection refusal, but a
/// lost or failed response. Callers that count possibly stored handshakes need this.
#[derive(Debug, Clone, Copy)]
pub struct MaybeDelivered;
impl std::fmt::Display for MaybeDelivered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTPS request may have reached the relay")
    }
}
fn delivery(error: anyhow::Error, maybe: bool) -> anyhow::Error {
    if maybe && !error.is::<MaybeDelivered>() {
        error.context(MaybeDelivered)
    } else {
        error
    }
}
const RETRY_FIRST: Duration = Duration::from_millis(100);
const RETRY_LONGEST: Duration = Duration::from_secs(2);
const RETRY_AFTER_LONGEST: Duration = Duration::from_secs(5);
/// Jittered exponential retry delay; far below the 23 s verified-peer deadline.
pub struct Backoff(Duration);
impl Default for Backoff {
    fn default() -> Self {
        Self(RETRY_FIRST)
    }
}
impl Backoff {
    pub fn reset(&mut self) {
        self.0 = RETRY_FIRST;
    }
    pub async fn wait(&mut self, error: &anyhow::Error, stop: &CancellationToken) -> Result<()> {
        let mut seed = [0; 4];
        getrandom::getrandom(&mut seed).map_err(|_| Diagnostic::err("OS randomness failed"))?;
        // Between half and all of the current step, so endpoints do not retry in lockstep.
        let step = self
            .0
            .mul_f64(0.5 + f64::from(u32::from_le_bytes(seed)) / f64::from(u32::MAX) / 2.0);
        let hint = error
            .downcast_ref::<RetryAfter>()
            .map_or(Duration::ZERO, |hint| hint.0);
        self.0 = (self.0 * 2).min(RETRY_LONGEST);
        tokio::select! {
            _ = stop.cancelled() => Err(Failure::Network.into()),
            _ = tokio::time::sleep(step.max(hint)) => Ok(()),
        }
    }
}
/// What this client learned about its relay, shared by all its requests.
///
/// A relay may announce `X-K-Lani-Relay: wait=<seconds>; writes=<n>` (the PHP relay does,
/// because a small web host has few workers). The client never waits longer or writes in
/// parallel more than announced, halves its write parallelism on overload and grows it
/// back slowly, and pauses longer when empty answers come back early (a relay that may
/// not hold a request open), so it never hammers the site into blocking it.
pub struct Tuning {
    advised_wait: AtomicU64,
    advised_writes: AtomicUsize,
    writes: AtomicUsize,
    streak: AtomicUsize,
    quiet_ms: AtomicU64,
}
impl Default for Tuning {
    fn default() -> Self {
        Self {
            advised_wait: AtomicU64::new(u64::MAX),
            advised_writes: AtomicUsize::new(PIPELINE),
            writes: AtomicUsize::new(PIPELINE),
            streak: AtomicUsize::new(0),
            quiet_ms: AtomicU64::new(0),
        }
    }
}
/// The longest pause between polls a relay that answers at once can cause.
const QUIET_LONGEST_MS: u64 = 2000;
const QUIET_FIRST_MS: u64 = 200;
impl Tuning {
    fn advise(&self, header: Option<&str>) {
        for item in header.unwrap_or("").split(';') {
            let Some((key, value)) = item.trim().split_once('=') else {
                continue;
            };
            let Ok(value) = value.trim().parse::<u64>() else {
                continue;
            };
            match key.trim() {
                "wait" => self.advised_wait.store(value, Ordering::Relaxed),
                "writes" => self.advised_writes.store(
                    usize::try_from(value).unwrap_or(1).clamp(1, PIPELINE),
                    Ordering::Relaxed,
                ),
                _ => {}
            }
        }
    }
    /// Additive increase: one more parallel write after a window's worth of successes.
    fn succeeded(&self) {
        let writes = self.writes.load(Ordering::Relaxed);
        if self.streak.fetch_add(1, Ordering::Relaxed) + 1 >= writes && writes < PIPELINE {
            self.writes.store(writes + 1, Ordering::Relaxed);
            self.streak.store(0, Ordering::Relaxed);
        }
    }
    /// Multiplicative decrease on overload, timeouts or refusals.
    fn overloaded(&self) {
        let writes = self.writes.load(Ordering::Relaxed);
        self.writes.store((writes / 2).max(1), Ordering::Relaxed);
        self.streak.store(0, Ordering::Relaxed);
    }
}
#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
    /// Tried first when `http3` is set; TCP (HTTP/2 or HTTP/1.1) is the fallback.
    quic: Option<Arc<super::quic::Client>>,
    origins: Vec<String>,
    token: String,
    wait: u64,
    tuning: Arc<Tuning>,
    /// How writes travel, raised for the rest of this client when a web firewall refuses
    /// one: 0 PUT, 1 POST, 2 POST with the record wrapped in JSON.
    writes: Arc<std::sync::atomic::AtomicU8>,
}
/// A relay response over TCP or QUIC; the transport reads only its status and body.
pub enum Reply {
    Tcp(reqwest::Response),
    Quic(
        u16,
        Box<http::HeaderMap>,
        Box<super::quic::Stream>,
        Arc<super::quic::Client>,
    ),
}
impl Reply {
    pub fn status(&self) -> u16 {
        match self {
            Self::Tcp(response) => response.status().as_u16(),
            Self::Quic(status, ..) => *status,
        }
    }
    fn header(&self, name: &str) -> Option<String> {
        let headers = match self {
            Self::Tcp(response) => response.headers(),
            Self::Quic(_, headers, ..) => headers,
        };
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    }
    pub async fn chunk(&mut self) -> Result<Option<Bytes>> {
        match self {
            Self::Tcp(response) => response.chunk().await.map_err(|_| Failure::Network.into()),
            Self::Quic(_, _, stream, client) => {
                // Mirrors the TCP client's overall request timeout.
                let chunk =
                    tokio::time::timeout(Duration::from_secs(35), super::quic::chunk(stream))
                        .await
                        .map_err(|_| Failure::Network)
                        .and_then(|chunk| chunk.map_err(|_| Failure::Network));
                if chunk.is_err() {
                    client.failed();
                }
                Ok(chunk?)
            }
        }
    }
}
impl Http {
    pub fn new(options: &Options) -> Result<Self> {
        options.validate()?;
        let mut client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(35));
        if options.http1_only {
            client = client.http1_only();
        }
        if let Some(proxy) = &options.proxy_url {
            client = client.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| Diagnostic::err("invalid explicit HTTPS proxy configuration"))?,
            );
        }
        let mut ca = None;
        if let Some(path) = &options.ca_cert_file {
            let pem = super::public_file(path, 128 * 1024)?;
            let roots = reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|_| Diagnostic::err("invalid HTTPS CA certificate"))?;
            if roots.is_empty() {
                return Err(Diagnostic::err("HTTPS CA certificate file is empty"));
            }
            for root in roots {
                client = client.add_root_certificate(root);
            }
            ca = Some(pem);
        }
        let quic = if options.http3 {
            Some(Arc::new(
                super::quic::Client::new(&options.relays[0], ca.as_deref())
                    .map_err(|_| Diagnostic::err("HTTP/3 client configuration failed"))?,
            ))
        } else {
            None
        };
        Ok(Self {
            client: client
                .build()
                .map_err(|_| Diagnostic::err("HTTPS client configuration failed"))?,
            quic,
            origins: options
                .relays
                .iter()
                .map(|url| url.trim_end_matches('/').to_owned())
                .collect(),
            token: secret(&options.admission_token_file)?,
            wait: options.poll_wait_seconds,
            tuning: Arc::default(),
            writes: Arc::new(std::sync::atomic::AtomicU8::new(u8::from(
                options.post_writes,
            ))),
        })
    }
    /// Seconds a read may wait at the relay: configured, but never more than announced.
    pub fn wait(&self) -> u64 {
        self.wait
            .min(self.tuning.advised_wait.load(Ordering::Relaxed))
    }
    /// Writes this client keeps in flight now (1..=PIPELINE).
    pub fn writes(&self) -> usize {
        self.tuning
            .writes
            .load(Ordering::Relaxed)
            .min(self.tuning.advised_writes.load(Ordering::Relaxed))
            .max(1)
    }
    pub fn relay_index(&self, sequence: u64) -> usize {
        // The configured relay count is one. Keep addressing sequence-based for the specified later extension.
        (sequence % self.origins.len() as u64) as usize
    }
    /// Whether the last request went over QUIC; tests and probes report it.
    pub fn on_quic(&self) -> bool {
        self.quic.as_ref().is_some_and(|quic| quic.usable())
    }
    async fn exchange(
        &self,
        method: reqwest::Method,
        path: &str,
        sequence: u64,
        body: Option<Bytes>,
        json: bool,
    ) -> Result<Reply> {
        let (body, content_type) = match body {
            Some(bytes) if json => (
                Some(Bytes::from(format!(
                    "{{\"data\":\"{}\"}}",
                    base64::Engine::encode(
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                        &bytes
                    )
                ))),
                "application/json",
            ),
            body => (body, "application/octet-stream"),
        };
        let mut maybe = false;
        if let Some(quic) = self.quic.as_ref().filter(|quic| quic.usable()) {
            match quic
                .send(&method, path, &self.token, body.clone(), content_type)
                .await
            {
                Ok((status, headers, stream)) => {
                    return Ok(Reply::Quic(
                        status,
                        Box::new(headers),
                        Box::new(stream),
                        quic.clone(),
                    ));
                }
                // Same request over TCP at once: a resent PUT carries identical sealed bytes.
                Err(_) => {
                    quic.failed();
                    maybe = body.is_some();
                }
            }
        }
        let mut request = self
            .client
            .request(
                method,
                format!("{}{path}", self.origins[self.relay_index(sequence)]),
            )
            .bearer_auth(&self.token)
            .header("cache-control", "no-store, no-transform");
        if let Some(body) = body {
            request = request.header("content-type", content_type).body(body);
        }
        let response = request.send().await.map_err(|error| {
            // Refused before the request left: the relay cannot have it, unless a QUIC
            // attempt of the same write got further.
            delivery(Failure::Network.into(), maybe || !error.is_connect())
        })?;
        Ok(Reply::Tcp(response))
    }
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        sequence: u64,
        body: Option<Bytes>,
        stop: &CancellationToken,
        deadline: Instant,
    ) -> Result<Reply> {
        use std::sync::atomic::Ordering;
        let write = method == reqwest::Method::PUT;
        let exchange = async {
            loop {
                let stage = if write {
                    self.writes.load(Ordering::Relaxed)
                } else {
                    0
                };
                let selected = if stage > 0 {
                    reqwest::Method::POST
                } else {
                    method.clone()
                };
                let reply = self
                    .exchange(selected, path, sequence, body.clone(), stage > 1)
                    .await?;
                // A web firewall refusing the method (PUT) or the binary type: the same
                // sealed bytes go the next way, from now on. Relays answer a bad token with
                // 401, so a 403 here comes from such a rule.
                let refused = match stage {
                    0 => matches!(reply.status(), 403 | 405 | 501),
                    1 => matches!(reply.status(), 403 | 415),
                    _ => false,
                };
                if !(write && refused) {
                    return Ok::<_, anyhow::Error>(reply);
                }
                self.writes.store(stage + 1, Ordering::Relaxed);
            }
        };
        let reply = tokio::select! {
            _ = stop.cancelled() => return Err(Failure::Network.into()),
            // A timeout says nothing about whether the relay already stored the request.
            result = tokio::time::timeout_at(deadline, exchange) => result
                .map_err(|_| delivery(Failure::Network.into(), true))??,
        };
        self.tuning
            .advise(reply.header("x-k-lani-relay").as_deref());
        match reply.status() {
            200 | 204 => Ok(reply),
            429 | 503 => {
                let hint = reply
                    .header("retry-after")
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .map_or(Duration::ZERO, |seconds| {
                        Duration::from_secs(seconds).min(RETRY_AFTER_LONGEST)
                    });
                Err(anyhow::Error::new(Failure::Network).context(RetryAfter(hint)))
            }
            // A proxy's timeout or error says nothing about whether the relay stored it.
            408 | 500..=599 => Err(delivery(Failure::Network.into(), true)),
            410 => Err(Failure::Gone.into()),
            _ => Err(Failure::Denied.into()),
        }
    }
    pub async fn get(
        &self,
        path: &str,
        sequence: u64,
        stop: &CancellationToken,
        deadline: Instant,
    ) -> Result<Reply> {
        self.send(reqwest::Method::GET, path, sequence, None, stop, deadline)
            .await
    }
    pub async fn get_bytes(
        &self,
        path: &str,
        sequence: u64,
        maximum: usize,
        stop: &CancellationToken,
        deadline: Instant,
    ) -> Result<Option<Bytes>> {
        let started = Instant::now();
        let mut response = self.get(path, sequence, stop, deadline).await?;
        if response.status() == 204 {
            self.idle(stop, started).await?;
            return Ok(None);
        }
        self.active();
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {
                _ = stop.cancelled() => return Err(Failure::Network.into()),
                chunk=tokio::time::timeout_at(deadline,response.chunk()) => chunk
                    .map_err(|_| Failure::Network)?.map_err(|_| Failure::Network)?,
            };
            let Some(chunk) = chunk else { break };
            if bytes.len() + chunk.len() > maximum {
                return Err(Failure::Invalid.into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(Bytes::from(bytes)))
    }
    pub async fn put(
        &self,
        path: &str,
        sequence: u64,
        bytes: Bytes,
        stop: &CancellationToken,
        deadline: Instant,
    ) -> Result<()> {
        let mut backoff = Backoff::default();
        let mut maybe = false;
        loop {
            if Instant::now() >= deadline {
                return Err(delivery(Failure::Network.into(), maybe));
            }
            let sent = self
                .send(
                    reqwest::Method::PUT,
                    path,
                    sequence,
                    Some(bytes.clone()),
                    stop,
                    deadline,
                )
                .await;
            match sent {
                Ok(response) if response.status() == 204 => {
                    self.tuning.succeeded();
                    return Ok(());
                }
                Ok(_) => return Err(Failure::Invalid.into()),
                Err(error) if error.downcast_ref::<Failure>() == Some(&Failure::Network) => {
                    self.tuning.overloaded();
                    maybe |= error.is::<MaybeDelivered>();
                    backoff
                        .wait(&error, stop)
                        .await
                        .map_err(|error| delivery(error, maybe))?
                }
                Err(error) => return Err(error),
            }
        }
    }
    /// The pause after an empty answer to a read that started at `started`. An answer that
    /// came back well before the wait ran out means the relay did not hold the request
    /// open (busy or short waits): pause longer, up to two seconds, instead of polling hot.
    pub async fn idle(&self, stop: &CancellationToken, started: Instant) -> Result<()> {
        let early = started.elapsed() < Duration::from_secs(self.wait()) / 2 || self.wait() == 0;
        let pause = if early {
            let next = (self.tuning.quiet_ms.load(Ordering::Relaxed) * 2)
                .clamp(QUIET_FIRST_MS, QUIET_LONGEST_MS);
            self.tuning.quiet_ms.store(next, Ordering::Relaxed);
            next
        } else {
            self.tuning.quiet_ms.store(0, Ordering::Relaxed);
            BATCH_MS
        };
        tokio::select! {
            _=stop.cancelled()=>Err(Failure::Network.into()),
            _=tokio::time::sleep(Duration::from_millis(pause))=>Ok(()),
        }
    }
    /// Data arrived: the next empty answer starts the pause from the shortest step again.
    pub fn active(&self) {
        self.tuning.quiet_ms.store(0, Ordering::Relaxed);
    }
}

pub fn packets(bytes: &[u8]) -> Result<Vec<Bytes>> {
    if bytes.len() > WINDOW * (MAX_PACKET_BYTES + 4) {
        return Err(Failure::Invalid.into());
    }
    let mut rest = bytes;
    let mut output = Vec::new();
    while !rest.is_empty() {
        if rest.len() < 4 || output.len() >= WINDOW {
            return Err(Failure::Invalid.into());
        }
        let length = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        if !(28..=MAX_PACKET_BYTES).contains(&length) || rest.len() < 4 + length {
            return Err(Failure::Invalid.into());
        }
        output.push(Bytes::copy_from_slice(&rest[4..4 + length]));
        rest = &rest[4 + length..];
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn retries_back_off_exponentially_with_jitter_and_honor_a_bounded_retry_after() {
        let stop = CancellationToken::new();
        let plain = anyhow::Error::from(Failure::Network);
        let mut backoff = Backoff::default();
        let mut longest = Duration::ZERO;
        for _ in 0..8 {
            let began = Instant::now();
            backoff.wait(&plain, &stop).await.unwrap();
            longest = longest.max(began.elapsed());
        }
        assert!(longest >= RETRY_LONGEST / 2 && longest <= RETRY_LONGEST);
        backoff.reset();
        let began = Instant::now();
        backoff.wait(&plain, &stop).await.unwrap();
        assert!(began.elapsed() <= RETRY_FIRST);
        for (hint, expected) in [(3, 3), (3600, 5)] {
            let hinted = anyhow::Error::new(Failure::Network).context(RetryAfter(
                Duration::from_secs(hint).min(RETRY_AFTER_LONGEST),
            ));
            assert_eq!(hinted.downcast_ref::<Failure>(), Some(&Failure::Network));
            let began = Instant::now();
            backoff.wait(&hinted, &stop).await.unwrap();
            assert_eq!(began.elapsed().as_secs(), expected);
        }
    }

    #[test]
    fn a_relay_s_announced_limits_and_overload_shape_the_client() {
        let tuning = Tuning::default();
        assert_eq!(tuning.writes.load(Ordering::Relaxed), PIPELINE);
        // Unknown or malformed parts are ignored; writes stay within 1..=PIPELINE.
        tuning.advise(Some("wait=3; writes=2; extra=1; broken"));
        assert_eq!(tuning.advised_wait.load(Ordering::Relaxed), 3);
        assert_eq!(tuning.advised_writes.load(Ordering::Relaxed), 2);
        tuning.advise(Some("writes=0"));
        assert_eq!(tuning.advised_writes.load(Ordering::Relaxed), 1);
        tuning.advise(Some("writes=999"));
        assert_eq!(tuning.advised_writes.load(Ordering::Relaxed), PIPELINE);
        // Overload halves, down to one; successes add one per window's worth.
        tuning.overloaded();
        tuning.overloaded();
        tuning.overloaded();
        tuning.overloaded();
        assert_eq!(tuning.writes.load(Ordering::Relaxed), 1);
        tuning.succeeded();
        assert_eq!(tuning.writes.load(Ordering::Relaxed), 2);
        tuning.succeeded();
        assert_eq!(tuning.writes.load(Ordering::Relaxed), 2);
        tuning.succeeded();
        assert_eq!(tuning.writes.load(Ordering::Relaxed), 3);
    }
}
