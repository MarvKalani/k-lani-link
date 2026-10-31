//! Bounded, short-lived SDP mailbox; never carries application frames.
use crate::config::{Config, token_matches};
use anyhow::Result;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use k_lani_link_protocol::{EndpointRole, MAX_SIGNAL_BYTES, Signal, SignalContext, valid_room};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
type Mailboxes = HashMap<(String, String), (Instant, Signal)>;
#[derive(Clone)]
struct Mailbox {
    token: String,
    entries: Arc<Mutex<Mailboxes>>,
}

pub fn router(token: String) -> Router {
    Router::new()
        .route("/v1/signal/{room}/{kind}", get(read).put(write))
        .layer(DefaultBodyLimit::max(MAX_SIGNAL_BYTES))
        .with_state(Mailbox {
            token,
            entries: Arc::new(Mutex::new(HashMap::new())),
        })
}
fn authorized(headers: &HeaderMap, token: &str) -> bool {
    token_matches(
        headers.get("authorization").and_then(|h| h.to_str().ok()),
        token,
    )
}
fn valid_path(room: &str, kind: &str) -> bool {
    valid_room(room) && matches!(kind, "offer" | "answer")
}
async fn read(
    State(state): State<Mailbox>,
    Path((room, kind)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !valid_path(&room, &kind) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut entries = state.entries.lock().unwrap();
    entries.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(120));
    let mut response = match entries.get(&(room, kind)) {
        Some((_, value)) => Json(value.clone()).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
        .headers_mut()
        .insert("connection", "close".parse().unwrap());
    response
}
async fn write(
    State(state): State<Mailbox>,
    Path((room, kind)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !valid_path(&room, &kind) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(value) = serde_json::from_slice::<Signal>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if value.context.room != room
        || value.context.kind != kind
        || value.ciphertext.len() > 34 * 1024
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let mut entries = state.entries.lock().unwrap();
    entries.retain(|_, (time, _)| time.elapsed() < Duration::from_secs(120));
    if entries.len() >= 128 && !entries.contains_key(&(room.clone(), kind.clone())) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    entries.insert((room, kind), (Instant::now(), value));
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert("connection", "close".parse().unwrap());
    response
}

pub struct Signaling {
    client: reqwest::Client,
    config: Config,
}
impl Signaling {
    pub fn new(config: &Config) -> Result<Self> {
        // No persistent intermediary connection, even during setup. No system proxy.
        let client = reqwest::Client::builder()
            .http1_only()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .pool_max_idle_per_host(0)
            .timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            client,
            config: config.clone(),
        })
    }
    fn url(&self, kind: &str) -> String {
        format!(
            "{}/v1/signal/{}/{kind}",
            self.config.signaling_url.trim_end_matches('/'),
            self.config.room
        )
    }
    pub async fn put(&self, value: &Signal) -> Result<()> {
        self.client
            .put(self.url(&value.context.kind))
            .bearer_auth(&self.config.signaling_token)
            .header("connection", "close")
            .json(value)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
    pub async fn get(&self, kind: &str) -> Result<Option<Signal>> {
        let response = self
            .client
            .get(self.url(kind))
            .bearer_auth(&self.config.signaling_token)
            .header("connection", "close")
            .send()
            .await?
            .error_for_status()?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > MAX_SIGNAL_BYTES {
                return Err(crate::pairing::PairingRejected.into());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Some(
            serde_json::from_slice(&body).map_err(|_| crate::pairing::PairingRejected)?,
        ))
    }
}

pub fn seal(
    config: &Config,
    session: &str,
    kind: &str,
    sdp: &str,
    endpoint: EndpointRole,
) -> Result<Signal> {
    let mut nonce = [0u8; 24];
    getrandom::getrandom(&mut nonce).map_err(|e| anyhow::anyhow!("OS randomness failed: {e}"))?;
    Signal::seal(
        SignalContext {
            room: config.room.clone(),
            session: session.into(),
            kind: kind.into(),
            issued_at: now(),
        },
        sdp,
        endpoint,
        &config.pair_key,
        &nonce,
    )
    .map_err(anyhow::Error::msg)
}
