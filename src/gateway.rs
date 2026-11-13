use crate::{
    config::{Config, token_matches},
    peer,
    tunnel::Tunnel,
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use k_lani_link_protocol::{
    Api, CHUNK_BYTES, Frame, MAX_BODY_BYTES, MAX_CONTROL_BODY_BYTES, RESPONSE_WINDOW,
    UPLOAD_SECONDS, accepted_query, allowed_route, control_body_valid, decode_data,
    management_route, multipart_content_type, multipart_route, route_api,
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

mod local_audio;

#[derive(Clone)]
pub struct Link {
    pub tx: mpsc::Sender<Frame>,
    pub closed: CancellationToken,
    recovering: Arc<AtomicBool>,
    generation: u64,
}
struct Pending {
    generation: u64,
    tx: mpsc::Sender<Frame>,
}
#[derive(Clone)]
pub struct Gateway {
    config: Config,
    local_audio: Arc<local_audio::AudioRoutes>,
    audio_slot: Arc<Semaphore>,
    link: Arc<Mutex<Option<Link>>>,
    pending: Arc<Mutex<HashMap<u64, Pending>>>,
    slot: Arc<Semaphore>,
    control_slot: Arc<Semaphore>,
    management_token: Option<String>,
    upload_timeout: Duration,
    next: Arc<AtomicU64>,
    rejected: Arc<AtomicBool>,
    /// The HTTPS relay refused the admission token (expired, revoked or address rules).
    denied: Arc<AtomicBool>,
    code_used: Arc<AtomicBool>,
    session_ended: Arc<AtomicBool>,
    reconnecting: Arc<AtomicBool>,
    /// Since when the HTTPS relay has refused every request (Unix seconds, 0: it admits).
    refused: crate::mailbox::http::Refusals,
    linked: Arc<tokio::sync::Notify>,
}
/// A relay that refused every request this long is reported as full for this access,
/// not as a slow connection: waiting longer does not help, freeing a connection does.
const REFUSED_SECONDS: u64 = 15;
/// Each unanswered HTTPS hello may hold one of the host's `MAX_PENDING` attempts until a
/// handshake completes. Automatic retries stop one short, leaving one for an explicit restart.
const UNANSWERED_HELLO_LIMIT: usize = k_lani_link_protocol::MAX_PENDING - 1;
impl Gateway {
    pub fn new(config: Config) -> anyhow::Result<Self> {
        config.validate()?;
        let management_token = crate::management::token(&config)?;
        let capacity = config.max_concurrent_requests + config.max_queued_requests;
        let audio_slot = Arc::new(Semaphore::new(config.max_concurrent_requests));
        let local_audio = Arc::new(local_audio::AudioRoutes::new(&config.local_audio)?);
        Ok(Self {
            config,
            local_audio,
            audio_slot,
            link: Default::default(),
            pending: Default::default(),
            slot: Arc::new(Semaphore::new(capacity)),
            control_slot: Arc::new(Semaphore::new(1)),
            management_token,
            upload_timeout: Duration::from_secs(UPLOAD_SECONDS),
            next: Arc::new(AtomicU64::new(1)),
            rejected: Arc::new(AtomicBool::new(false)),
            denied: Arc::new(AtomicBool::new(false)),
            code_used: Arc::new(AtomicBool::new(false)),
            session_ended: Arc::new(AtomicBool::new(false)),
            reconnecting: Arc::new(AtomicBool::new(false)),
            refused: Default::default(),
            linked: Arc::new(tokio::sync::Notify::new()),
        })
    }
    /// A body that has not fully arrived by then ends the request and frees its slot.
    /// Tests shorten it; the default matches the host's own upload limit.
    pub fn with_upload_timeout(mut self, timeout: Duration) -> Self {
        self.upload_timeout = timeout;
        self
    }
    pub fn router(&self) -> Router {
        Router::new()
            .route("/health", get(health))
            .fallback(proxy)
            .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
            .with_state(self.clone())
    }
    pub fn is_connected(&self) -> bool {
        self.link
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|l| !l.closed.is_cancelled())
    }
    pub async fn attach(&self, peer: impl Into<Tunnel>) {
        let mut peer = peer.into();
        let closed = peer.closed().clone();
        let generation = self.next.fetch_add(1, Ordering::Relaxed);
        let link = Link {
            tx: peer.tx().clone(),
            closed: peer.closed().clone(),
            recovering: peer.recovering().clone(),
            generation,
        };
        *self.link.lock().unwrap() = Some(link);
        self.linked.notify_waiters();
        loop {
            let frame =
                tokio::select! { _ = closed.cancelled() => break, frame = peer.receive() => frame };
            let Some(frame) = frame else { break };
            let tx = self
                .pending
                .lock()
                .unwrap()
                .get(&frame.id())
                .filter(|p| p.generation == generation)
                .map(|p| p.tx.clone());
            if let Some(tx) = tx
                && tx.try_send(frame).is_err()
            {
                peer.closed().cancel();
                break;
            }
        }
        {
            let mut link = self.link.lock().unwrap();
            if link.as_ref().is_some_and(|l| l.generation == generation) {
                *link = None;
            }
        }
        // With resume, a request leaves only through its guard: it may continue in the
        // next session.
        if self.config.resume_seconds == 0 {
            let mut pending = self.pending.lock().unwrap();
            pending.retain(|_, p| p.generation != generation);
        }
        peer.close().await;
    }
    pub async fn maintain(&self, stop: CancellationToken, state_dir: std::path::PathBuf) {
        if self.config.transport == crate::config::Transport::Https {
            self.maintain_https(stop).await;
            return;
        }
        let Ok(mut lease) =
            crate::pairing::PairingLease::reserve(&state_dir, &self.config.pair_key)
        else {
            self.code_used.store(true, Ordering::SeqCst);
            return;
        };
        loop {
            let result = tokio::select! { _ = stop.cancelled() => return, result = peer::connect(&self.config) => result };
            match result {
                Ok(peer) => {
                    if lease.consume().is_err() {
                        self.code_used.store(true, Ordering::SeqCst);
                        return;
                    }
                    self.code_used.store(true, Ordering::SeqCst);
                    tokio::select! { _ = stop.cancelled() => return, _ = self.attach(peer) => {} }
                    return;
                }
                Err(error) if error.is::<crate::pairing::PairingRejected>() => {
                    self.rejected.store(true, Ordering::SeqCst);
                    return;
                }
                Err(_) => {}
            }
            tokio::select! { _ = stop.cancelled() => return, _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
        }
    }
    async fn maintain_https(&self, stop: CancellationToken) {
        use crate::mailbox::{
            http::{Backoff, Failure},
            transport::UnansweredHello,
        };
        let result = async {
            let options = self
                .config
                .https
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("HTTPS settings required"))?;
            let store = Arc::new(Mutex::new(crate::chain_state::Store::open(
                &options.state_dir,
                k_lani_link_protocol::EndpointRole::Client,
            )?));
            store.lock().unwrap().client(crate::signal::now())?;
            let mut backoff = Backoff::default();
            let mut unanswered = 0;
            loop {
                let connected = tokio::select! {
                    biased;
                    _ = stop.cancelled() => return Ok(None),
                    connected = crate::mailbox::transport::connect_with(options, store.clone(), stop.clone(), self.refused.clone()) => connected,
                };
                match connected {
                    Ok(peer) => {
                        unanswered = 0;
                        backoff.reset();
                        let rejected = peer.rejected.clone();
                        self.reconnecting.store(false, Ordering::SeqCst);
                        // Stopping closes the session through attach(), which waits for the
                        // session's final activity checkpoint instead of leaving it running.
                        let closed = peer.closed.clone();
                        let stopping = stop.clone();
                        let closer = tokio::spawn(async move {
                            stopping.cancelled().await;
                            closed.cancel();
                        });
                        self.attach(peer).await;
                        closer.abort();
                        if stop.is_cancelled() {
                            return Ok(None);
                        }
                        if rejected.load(Ordering::SeqCst) {
                            return Err(crate::pairing::PairingRejected.into());
                        }
                        // attach() already failed the old session's requests. Negotiate a
                        // fresh session at once; no request or stream is replayed.
                        self.reconnecting.store(true, Ordering::SeqCst);
                    }
                    Err(error)
                        if matches!(
                            error.downcast_ref::<Failure>(),
                            Some(Failure::Network | Failure::Gone)
                        ) =>
                    {
                        if error.is::<UnansweredHello>() {
                            unanswered += 1;
                            if unanswered == UNANSWERED_HELLO_LIMIT {
                                return Ok(Some(()));
                            }
                        }
                        if let Err(error) = backoff.wait(&error, &stop).await {
                            return if stop.is_cancelled() {
                                Ok(None)
                            } else {
                                Err(error)
                            };
                        }
                    }
                    Err(_) if stop.is_cancelled() => return Ok(None),
                    Err(error) => return Err(error),
                }
            }
        };
        let result = match result.await {
            Ok(None) => return,
            Ok(Some(())) => Ok(()),
            Err(error) => Err(error),
        };
        self.reconnecting.store(false, Ordering::SeqCst);
        self.session_ended.store(true, Ordering::SeqCst);
        self.linked.notify_waiters();
        if let Err(error) = result {
            self.rejected.store(true, Ordering::SeqCst);
            if error.chain().any(|cause| {
                cause.downcast_ref::<crate::mailbox::http::Failure>()
                    == Some(&crate::mailbox::http::Failure::Denied)
            }) {
                self.denied.store(true, Ordering::SeqCst);
            }
            if crate::diagnostic::printable(&error).is_some_and(|text| {
                [
                    crate::chain_state::MISSING,
                    crate::chain_state::REVOKED,
                    crate::chain_state::EXPIRED,
                    crate::chain_state::DAMAGED,
                    &k_lani_link_protocol::HelloError::Stale.to_string(),
                ]
                .contains(&text.as_str())
            }) {
                self.code_used.store(true, Ordering::SeqCst);
            }
        }
        // Automatic sessions stopped: a permanent failure, or the pending-attempt guard.
    }
}

impl Gateway {
    /// Resume enabled: wait for the next session within `resume_seconds` and the request
    /// deadline, move the request to it and ask the host to continue the response from
    /// the `received` Data frame. Never resubmits the request itself.
    async fn resume(
        &self,
        link: &mut Link,
        id: u64,
        response: bool,
        received: u64,
        deadline: tokio::time::Instant,
    ) -> bool {
        if self.config.resume_seconds == 0 {
            return false;
        }
        let until = deadline
            .min(tokio::time::Instant::now() + Duration::from_secs(self.config.resume_seconds));
        let Some(next) = self.relink(link.generation, until).await else {
            return false;
        };
        // Route the host's retransmission to this request before asking for it.
        match self.pending.lock().unwrap().get_mut(&id) {
            Some(pending) => pending.generation = next.generation,
            None => return false,
        }
        if next
            .tx
            .send(Frame::Resume {
                id,
                response,
                received,
            })
            .await
            .is_err()
        {
            return false;
        }
        *link = next;
        true
    }
    async fn relink(&self, old: u64, until: tokio::time::Instant) -> Option<Link> {
        loop {
            let notified = self.linked.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let current = self.link.lock().unwrap().clone();
            if let Some(link) = current.filter(|l| l.generation != old && !l.closed.is_cancelled())
            {
                return Some(link);
            }
            if self.session_ended.load(Ordering::SeqCst) {
                return None;
            }
            tokio::time::timeout_at(until, notified).await.ok()?;
        }
    }
}

struct RequestGuard {
    id: u64,
    gateway: Gateway,
    link: Link,
    permit: Option<OwnedSemaphorePermit>,
    finished: bool,
}
impl RequestGuard {
    fn finish(&mut self) {
        self.finished = true;
    }
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.gateway.pending.lock().unwrap().remove(&self.id);
        if !self.finished {
            match self.link.tx.try_send(Frame::Cancel { id: self.id }) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(frame)) => {
                    let link = self.link.clone();
                    let permit = self.permit.take();
                    tokio::spawn(async move {
                        let _hold_capacity = permit;
                        tokio::select! {
                            _ = link.closed.cancelled() => {},
                            sent = tokio::time::timeout(Duration::from_secs(2), link.tx.send(frame)) => {
                                if !matches!(sent, Ok(Ok(()))) { link.closed.cancel(); }
                            }
                        }
                    });
                }
                Err(mpsc::error::TrySendError::Closed(_)) => self.link.closed.cancel(),
            }
        }
    }
}
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({"error":{"message":message,"type":"link_error","code":status.as_u16()}}))).into_response()
}
/// `Authorization: Bearer` (OpenAI clients, Claude Code with `ANTHROPIC_AUTH_TOKEN`) or
/// `x-api-key` (Claude Code with `ANTHROPIC_API_KEY`). Neither is forwarded.
fn auth(headers: &HeaderMap, cfg: &Config) -> bool {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|key| format!("Bearer {key}"));
    token_matches(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        &cfg.api_key,
    ) | token_matches(key.as_deref(), &cfg.api_key)
}
/// Gateway errors in the shape the calling API's clients understand.
fn api_error(api: Option<Api>, status: StatusCode, message: &str) -> Response {
    if api != Some(Api::Anthropic) {
        return error(status, message);
    }
    let kind = match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::PAYLOAD_TOO_LARGE => "request_too_large",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::BAD_REQUEST => "invalid_request_error",
        _ => "api_error",
    };
    (
        status,
        Json(serde_json::json!({"type":"error","error":{"type":kind,"message":message}})),
    )
        .into_response()
}
fn relay_busy(refused: &crate::mailbox::http::Refusals, now: u64) -> bool {
    let since = refused.load(Ordering::Relaxed);
    since != 0 && now.saturating_sub(since) >= REFUSED_SECONDS
}
async fn health(State(gateway): State<Gateway>, headers: HeaderMap) -> Response {
    if !auth(&headers, &gateway.config) {
        return error(StatusCode::UNAUTHORIZED, "invalid k-lani-link API key");
    }
    let (active, recovering) = gateway
        .link
        .lock()
        .unwrap()
        .as_ref()
        .filter(|link| !link.closed.is_cancelled())
        .map_or((false, false), |link| {
            (true, link.recovering.load(Ordering::SeqCst))
        });
    Json(serde_json::json!({
        "connected": active && !recovering,
        "recovering": recovering,
        "pairing_rejected": gateway.rejected.load(Ordering::SeqCst),
        "access_denied": gateway.denied.load(Ordering::SeqCst),
        "new_code_required": gateway.code_used.load(Ordering::SeqCst) && !active,
        "reconnecting": gateway.reconnecting.load(Ordering::SeqCst) && !active,
        "relay_busy": !active && relay_busy(&gateway.refused, crate::signal::now()),
        "reconnect_required":gateway.config.transport==crate::config::Transport::Https && gateway.session_ended.load(Ordering::SeqCst) && !active,
        "relay_policy": gateway.config.relay,
        "transport": gateway.config.transport,
        "data_relay": gateway.config.transport == crate::config::Transport::Https,
        "local_audio": {
            "speech": gateway.config.local_audio.speech.is_some(),
            "transcriptions": gateway.config.local_audio.transcriptions.is_some()
        },
        "max_concurrent_requests": gateway.config.max_concurrent_requests,
        "max_queued_requests": gateway.config.max_queued_requests,
        "model_management": gateway.management_token.is_some()
    }))
    .into_response()
}
async fn proxy(State(gateway): State<Gateway>, request: Request) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let headers = request.headers();
    let control = management_route(method.as_str(), uri.path());
    let api = route_api(method.as_str(), uri.path());
    let authorized = if control {
        gateway.management_token.as_ref().is_some_and(|key| {
            token_matches(
                headers.get("authorization").and_then(|v| v.to_str().ok()),
                key,
            )
        })
    } else {
        auth(headers, &gateway.config)
    };
    if !authorized {
        return api_error(api, StatusCode::UNAUTHORIZED, "invalid API credential");
    }
    // Native API consumers do not send browser Origin. No ambient browser access.
    if headers.contains_key("origin") {
        return api_error(
            api,
            StatusCode::FORBIDDEN,
            "browser origins are not enabled in the native MVP",
        );
    }
    // Paths are matched exactly; a query is accepted only where an API's clients always
    // send one, and it is not forwarded.
    let query = uri.query();
    if query.is_some_and(|query| api.and_then(accepted_query) != Some(query))
        || !(allowed_route(method.as_str(), uri.path()) || control)
    {
        return api_error(api, StatusCode::NOT_FOUND, "route not available");
    }
    let content_type = if multipart_route(method.as_str(), uri.path()) {
        match headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
        {
            Some(value) if multipart_content_type(value) => Some(value.to_owned()),
            _ => {
                return api_error(
                    api,
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "multipart/form-data with a valid boundary is required",
                );
            }
        }
    } else {
        None
    };
    let local = gateway.local_audio.endpoint(method.as_str(), uri.path());
    let list_audio = gateway.local_audio.enabled()
        && method == axum::http::Method::GET
        && uri.path() == "/v1/models";
    let uses_tunnel = local.is_none() && !list_audio;
    if uses_tunnel && gateway.rejected.load(Ordering::SeqCst) {
        return api_error(
            api,
            StatusCode::SERVICE_UNAVAILABLE,
            "pairing rejected; check the connection code, then restart",
        );
    }
    if uses_tunnel && gateway.code_used.load(Ordering::SeqCst) && !gateway.is_connected() {
        return api_error(
            api,
            StatusCode::SERVICE_UNAVAILABLE,
            "connection code consumed; pair with a new code",
        );
    }
    let slots = if local.is_some() || list_audio && !gateway.is_connected() {
        &gateway.audio_slot
    } else if control {
        &gateway.control_slot
    } else {
        &gateway.slot
    };
    let Ok(permit) = slots.clone().try_acquire_owned() else {
        return api_error(
            api,
            StatusCode::TOO_MANY_REQUESTS,
            "request capacity exhausted",
        );
    };
    let link = gateway.link.lock().unwrap().clone();
    let link = link.filter(|l| !l.closed.is_cancelled());
    if link.is_none() && uses_tunnel {
        return api_error(
            api,
            StatusCode::SERVICE_UNAVAILABLE,
            "home peer is not connected",
        );
    }
    let deadline =
        tokio::time::Instant::now() + Duration::from_secs(gateway.config.request_timeout_seconds);
    let limit = if control {
        MAX_CONTROL_BODY_BYTES
    } else {
        MAX_BODY_BYTES
    };
    // The slot is already held while the body arrives: bound that wait like the host does,
    // so a stalled local client cannot occupy capacity for the whole request timeout.
    let upload_deadline = deadline.min(tokio::time::Instant::now() + gateway.upload_timeout);
    let body = match tokio::time::timeout_at(
        upload_deadline,
        axum::body::to_bytes(request.into_body(), limit),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => {
            return api_error(
                api,
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body unavailable or too large",
            );
        }
        Err(_) => return api_error(api, StatusCode::REQUEST_TIMEOUT, "request upload timed out"),
    };
    if control && !control_body_valid(method.as_str(), &body) {
        return api_error(
            api,
            StatusCode::BAD_REQUEST,
            "invalid model control command",
        );
    }
    let forward = Forward {
        path: uri.path(),
        method,
        body,
        content_type,
        api,
        deadline,
        permit,
    };
    if let Some(endpoint) = local {
        return endpoint
            .forward(
                forward.path,
                forward
                    .content_type
                    .as_deref()
                    .unwrap_or("application/json"),
                forward.body,
                forward.deadline,
                forward.permit,
            )
            .await;
    }
    let remote = async {
        match link {
            Some(link) => tunnel_request(&gateway, link, forward).await,
            None => api_error(
                api,
                StatusCode::SERVICE_UNAVAILABLE,
                "home peer is not connected",
            ),
        }
    };
    if list_audio {
        let (local, remote) = tokio::join!(gateway.local_audio.models(), remote);
        return merge_models(remote, local, &gateway.config.local_audio).await;
    }
    remote.await
}

async fn merge_models(
    remote: Response,
    mut local: Vec<serde_json::Value>,
    config: &crate::config::LocalAudio,
) -> Response {
    if local.is_empty() && !remote.status().is_success() {
        return remote;
    }
    let mut catalog = serde_json::json!({"object":"list"});
    if remote.status().is_success() {
        let remote = async {
            let bytes = axum::body::to_bytes(remote.into_body(), 1024 * 1024)
                .await
                .ok()?;
            let mut value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
            let models = value["data"].as_array_mut()?;
            Some((std::mem::take(models), value))
        }
        .await;
        if let Some((models, original)) = remote {
            let has_alias = original.get("models").is_some();
            for model in models {
                let capability = model["capability"].as_str();
                let replaced = capability == Some("speech") && config.speech.is_some()
                    || capability == Some("transcription") && config.transcriptions.is_some();
                if !replaced && local.iter().all(|known| known["id"] != model["id"]) {
                    local.push(model);
                }
            }
            catalog = original;
            if has_alias {
                catalog["models"] = serde_json::json!(local);
            }
        } else if local.is_empty() {
            return error(StatusCode::BAD_GATEWAY, "invalid upstream model list");
        }
    }
    catalog["data"] = serde_json::json!(local);
    ([("cache-control", "no-store")], Json(catalog)).into_response()
}

struct Forward<'a> {
    path: &'a str,
    method: axum::http::Method,
    body: Bytes,
    content_type: Option<String>,
    api: Option<Api>,
    deadline: tokio::time::Instant,
    permit: OwnedSemaphorePermit,
}

async fn tunnel_request(gateway: &Gateway, mut link: Link, request: Forward<'_>) -> Response {
    let Forward {
        path,
        method,
        body,
        content_type,
        api,
        deadline,
        permit,
    } = request;
    let id = gateway.next.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::channel(RESPONSE_WINDOW + 4);
    gateway.pending.lock().unwrap().insert(
        id,
        Pending {
            generation: link.generation,
            tx,
        },
    );
    let mut guard = RequestGuard {
        id,
        gateway: gateway.clone(),
        link: link.clone(),
        permit: Some(permit),
        finished: false,
    };
    let upload = async {
        link.tx
            .send(Frame::Request {
                id,
                method: method.to_string(),
                path: path.to_owned(),
                content_type,
            })
            .await?;
        for chunk in body.chunks(CHUNK_BYTES) {
            link.tx.send(Frame::data(id, chunk)).await?;
        }
        link.tx.send(Frame::RequestEnd { id }).await?;
        Ok::<_, anyhow::Error>(())
    };
    // An upload cut by a lost session is never resumed: the host discarded it.
    let first = match tokio::select! {
        _ = link.closed.cancelled() => return api_error(api, StatusCode::BAD_GATEWAY, "peer disconnected"),
        result = tokio::time::timeout_at(deadline, upload) => result,
    } {
        Ok(Ok(())) => loop {
            let next = tokio::select! {
                biased;
                result = tokio::time::timeout_at(deadline, rx.recv()) => Some(result),
                _ = link.closed.cancelled() => None,
            };
            match next {
                Some(result) => break result.map(Ok),
                None if gateway.resume(&mut link, id, false, 0, deadline).await => {
                    guard.link = link.clone();
                }
                None => return api_error(api, StatusCode::BAD_GATEWAY, "peer disconnected"),
            }
        },
        Ok(Err(error)) => Ok(Err(error)),
        Err(elapsed) => Err(elapsed),
    };
    let (status, content_type) = match first {
        Ok(Ok(Some(Frame::Response {
            status,
            content_type,
            ..
        }))) => (status, content_type),
        Ok(Ok(Some(Frame::Error { message, .. }))) => {
            return api_error(api, StatusCode::BAD_GATEWAY, &message);
        }
        Err(_) => {
            return api_error(
                api,
                StatusCode::GATEWAY_TIMEOUT,
                "upstream response timed out",
            );
        }
        _ => {
            return api_error(
                api,
                StatusCode::BAD_GATEWAY,
                "peer disconnected or invalid response",
            );
        }
    };
    let Ok(status) = StatusCode::from_u16(status) else {
        return api_error(api, StatusCode::BAD_GATEWAY, "invalid upstream status");
    };
    let gateway = gateway.clone();
    let stream = async_stream::try_stream! {
        // Moving the guard INTO the generator retains cancellation even before first poll.
        let _keep_guard = &mut guard;
        let mut received = 0u64;
        loop {
            // Frames that arrived before a loss are consumed first, so `received` is exact.
            let next = tokio::select! {
                biased;
                result = tokio::time::timeout_at(deadline, rx.recv()) => Some(result),
                _ = link.closed.cancelled() => None,
            };
            let next = match next {
                Some(result) => result.map_err(|_| std::io::Error::other("request timed out"))?,
                None => {
                    if gateway.resume(&mut link, id, true, received, deadline).await {
                        guard.link = link.clone();
                        continue;
                    }
                    Err(std::io::Error::other("peer disconnected"))?
                }
            };
            match next {
                Some(Frame::Data { data, .. }) => {
                    let bytes = decode_data(&data).map_err(std::io::Error::other)?;
                    yield Bytes::from(bytes);
                    received += 1;
                    // A credit lost with the session is restored by the host on resume.
                    if link.tx.send(Frame::Credit { id }).await.is_err() && gateway.config.resume_seconds == 0 {
                        Err(std::io::Error::other("peer disconnected"))?
                    }
                }
                Some(Frame::End { .. }) => { guard.finish(); break; }
                Some(Frame::Error { message, .. }) => Err(std::io::Error::other(message))?,
                _ => Err(std::io::Error::other("stream ended without terminal frame"))?,
            }
        }
    };
    let stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
    > = Box::pin(stream);
    let response = Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("cache-control", "no-store")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(stream));
    response.unwrap_or_else(|_| api_error(api, StatusCode::BAD_GATEWAY, "invalid upstream headers"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_audio_precedes_tunnel_and_never_falls_back() {
        use crate::config::LocalAudioEndpoint;
        use axum::routing::post;
        use k_lani_link_protocol::{SPEECH_PATH, TRANSCRIPTIONS_PATH};
        use tower::ServiceExt;

        const LOCAL_KEY: &str = "local-audio-test-credential";
        const FORM_TYPE: &str = "multipart/form-data; boundary=test";
        const FORM: &str = "--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.wav\"\r\n\r\naudio\r\n--test--\r\n";
        let audio = Router::new()
            .route(
                "/v1/models",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["authorization"], format!("Bearer {LOCAL_KEY}"));
                    Json(
                        serde_json::json!({"data":[{"id":"local-voice","capability":"speech"},
                    {"id":"local-stt","capability":"transcription"}]}),
                    )
                }),
            )
            .route(
                SPEECH_PATH,
                post(|headers: HeaderMap, body: Bytes| async move {
                    assert_eq!(headers["authorization"], format!("Bearer {LOCAL_KEY}"));
                    assert_eq!(headers["content-type"], "application/json");
                    assert_eq!(
                        body.as_ref(),
                        br#"{"model":"local-voice","input":"synthetic"}"#
                    );
                    ([("content-type", "audio/wav")], "wave")
                }),
            )
            .route(
                TRANSCRIPTIONS_PATH,
                post(|headers: HeaderMap, body: Bytes| async move {
                    assert_eq!(headers["authorization"], format!("Bearer {LOCAL_KEY}"));
                    assert_eq!(headers["content-type"], FORM_TYPE);
                    assert_eq!(body.as_ref(), FORM.as_bytes());
                    Json(serde_json::json!({"text":"local"}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = crate::config::tests::config();
        let endpoint = LocalAudioEndpoint {
            upstream: format!("http://{}", listener.local_addr().unwrap()),
            upstream_api_key: LOCAL_KEY.into(),
            upstream_socket: None,
        };
        config.local_audio.speech = Some(endpoint.clone());
        config.local_audio.transcriptions = Some(endpoint);
        let client_key = config.api_key.clone();
        let server = tokio::spawn(async move { axum::serve(listener, audio).await.unwrap() });
        let gateway = Gateway::new(config).unwrap();
        gateway.rejected.store(true, Ordering::SeqCst);
        let router = gateway.router();
        let request = |method, path, content_type, body: &'static str| {
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {client_key}"))
                .header("content-type", content_type)
                .body(Body::from(body))
                .unwrap()
        };
        let response = router
            .clone()
            .oneshot(request("GET", "/v1/models", "application/json", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let catalog: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(catalog["data"][0]["id"], "local-voice");
        assert_eq!(catalog["data"][1]["id"], "local-stt");
        assert_eq!(catalog["data"][1]["local"], true);

        let response = router
            .clone()
            .oneshot(request(
                "POST",
                SPEECH_PATH,
                "application/json",
                r#"{"model":"local-voice","input":"synthetic"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "audio/wav");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
            "wave"
        );
        let response = router
            .clone()
            .oneshot(request("POST", TRANSCRIPTIONS_PATH, FORM_TYPE, FORM))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
            r#"{"text":"local"}"#
        );
        let response = router
            .clone()
            .oneshot(request(
                "POST",
                "/v1/chat/completions",
                "application/json",
                "{}",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let merged = merge_models(
            Json(serde_json::json!({"object":"list","data":[
                {"id":"remote-chat"},{"id":"remote-voice","capability":"speech"},
                {"id":"local-voice","capability":"speech"}
            ]}))
            .into_response(),
            gateway.local_audio.models().await,
            &gateway.config.local_audio,
        )
        .await;
        let body = axum::body::to_bytes(merged.into_body(), 4096)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["data"].as_array().unwrap().len(), 3);
        assert_eq!(value["data"][2]["id"], "remote-chat");

        server.abort();
        let _ = server.await;
        let (tx, mut rx) = mpsc::channel(4);
        *gateway.link.lock().unwrap() = Some(Link {
            tx,
            closed: CancellationToken::new(),
            recovering: Arc::new(AtomicBool::new(false)),
            generation: 0,
        });
        let response = router
            .oneshot(request(
                "POST",
                SPEECH_PATH,
                "application/json",
                r#"{"model":"local-voice","input":"synthetic"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_relay_that_keeps_refusing_is_called_busy_only_after_a_while() {
        let refused = crate::mailbox::http::Refusals::default();
        assert!(!relay_busy(&refused, 1_000));
        refused.store(1_000, Ordering::Relaxed);
        assert!(!relay_busy(&refused, 1_000 + REFUSED_SECONDS - 1));
        assert!(relay_busy(&refused, 1_000 + REFUSED_SECONDS));
        // A clock that went back never reads as a long refusal.
        assert!(!relay_busy(&refused, 900));
    }

    async fn call(api: &str, path: &str, header: (&str, &str)) -> (StatusCode, serde_json::Value) {
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{api}{path}"))
            .header(header.0, header.1)
            .header("content-type", "application/json")
            .body(r#"{"model":"m"}"#)
            .send()
            .await
            .unwrap();
        let status = StatusCode::from_u16(response.status().as_u16()).unwrap();
        (status, response.json().await.unwrap())
    }

    #[tokio::test]
    async fn claude_code_credentials_and_its_one_query_reach_the_tunnel_check() {
        let config = crate::config::tests::config();
        let bearer = format!("Bearer {}", config.api_key);
        let key = config.api_key.clone();
        let router = Gateway::new(config).unwrap().router();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        // No tunnel yet: passing authentication and the route check ends at 503.
        for (path, header) in [
            ("/v1/messages?beta=true", ("x-api-key", key.as_str())),
            ("/v1/messages?beta=true", ("authorization", bearer.as_str())),
            ("/v1/messages", ("x-api-key", key.as_str())),
            (
                "/v1/messages/count_tokens?beta=true",
                ("x-api-key", key.as_str()),
            ),
        ] {
            let (status, body) = call(&gateway, path, header).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
            assert_eq!(body["type"], "error");
            assert_eq!(body["error"]["type"], "api_error");
        }
        let (status, body) = call(&gateway, "/v1/messages", ("x-api-key", "wrong")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["type"], "authentication_error");
        for path in [
            "/v1/messages?beta=false",
            "/v1/messages?beta=true&x=1",
            "/v1/chat/completions?beta=true",
        ] {
            let (status, _) = call(&gateway, path, ("x-api-key", key.as_str())).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        }
        let (status, body) = call(
            &gateway,
            "/v1/chat/completions",
            ("x-api-key", key.as_str()),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"]["type"], "link_error");
    }
}
