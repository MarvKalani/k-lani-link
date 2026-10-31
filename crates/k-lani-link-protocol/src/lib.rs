//! Runtime-independent wire protocol. Browser adapters own WebRTC and HTTP.
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
mod chain;
mod secure;
pub use chain::{
    Accepted, Chain, Established, HELLO_BYTES, HelloError, HostChains, Initiator, MAX_PENDING,
    MAX_RETIRED, PENDING_BYTES, Pending, REPLY_BYTES, RETIRED_GENERATION_BYTES, Retired,
    RetiredGeneration, Verdict, Via,
};
pub use secure::{MAX_BATCH_FRAMES, MAX_PACKET_BYTES, SecureSession};

pub const VERSION: u8 = 2;
pub const MAX_SDP_BYTES: usize = 16 * 1024;
pub const CHUNK_BYTES: usize = 8 * 1024;
pub const MAX_FRAME_BYTES: usize = 12 * 1024;
// Large coding contexts include UTF-8 text, tool schemas and JSON escaping.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
pub const RESPONSE_WINDOW: usize = 8;
pub const MAX_CONTROL_BODY_BYTES: usize = 1024;
/// How long a request body may take to arrive, on the local API and on the host.
pub const UPLOAD_SECONDS: u64 = 30;
pub const MAX_SIGNAL_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Frame {
    Ready,
    Request {
        id: u64,
        method: String,
        path: String,
    },
    Data {
        id: u64,
        data: String,
    },
    RequestEnd {
        id: u64,
    },
    Response {
        id: u64,
        status: u16,
        content_type: String,
    },
    End {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    Cancel {
        id: u64,
    },
    Credit {
        id: u64,
    },
    /// Sent only when resume is enabled: continue a response from an earlier session.
    /// `received` counts the Data frames that already reached the caller.
    Resume {
        id: u64,
        response: bool,
        received: u64,
    },
}

impl Frame {
    pub fn id(&self) -> u64 {
        match self {
            Self::Ready => 0,
            Self::Request { id, .. }
            | Self::Data { id, .. }
            | Self::RequestEnd { id }
            | Self::Response { id, .. }
            | Self::End { id }
            | Self::Error { id, .. }
            | Self::Cancel { id }
            | Self::Credit { id }
            | Self::Resume { id, .. } => *id,
        }
    }
    pub fn data(id: u64, data: &[u8]) -> Self {
        Self::Data {
            id,
            data: STANDARD.encode(data),
        }
    }
}

pub fn decode_data(data: &str) -> Result<Vec<u8>, String> {
    if data.len() > CHUNK_BYTES.div_ceil(3) * 4 {
        return Err("chunk too large".into());
    }
    let bytes = STANDARD.decode(data).map_err(|_| "invalid base64")?;
    if bytes.len() > CHUNK_BYTES {
        return Err("chunk too large".into());
    }
    Ok(bytes)
}

pub fn decode_frame(bytes: &[u8]) -> Result<Frame, String> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("frame too large".into());
    }
    let frame: Frame = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if let Frame::Data { data, .. } = &frame {
        decode_data(data)?;
    }
    Ok(frame)
}

/// A record of the mailbox transport: a JSON array of frames, each checked like a single one.
pub fn decode_batch(bytes: &[u8]) -> Result<Vec<Frame>, String> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("frame too large".into());
    }
    let frames: Vec<Frame> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if frames.is_empty() || frames.len() > MAX_BATCH_FRAMES {
        return Err("invalid frame batch".into());
    }
    for frame in &frames {
        if let Frame::Data { data, .. } = frame {
            decode_data(data)?;
        }
    }
    Ok(frames)
}

/// The model API a route belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    OpenAi,
    Anthropic,
}

/// Every route the tunnel carries. Bodies pass through unchanged, so a backend's own API
/// revisions need no change here; supporting another route is one row, versioned with this
/// crate. A host without the row answers 404, so an older host fails cleanly.
pub const ROUTES: &[(Api, &str, &str)] = &[
    (Api::OpenAi, "GET", "/v1/models"),
    (Api::OpenAi, "POST", "/v1/chat/completions"),
    (Api::OpenAi, "POST", "/v1/responses"),
    (Api::OpenAi, "POST", "/v1/embeddings"),
    (Api::Anthropic, "POST", "/v1/messages"),
    (Api::Anthropic, "POST", "/v1/messages/count_tokens"),
];

pub fn route_api(method: &str, path: &str) -> Option<Api> {
    ROUTES
        .iter()
        .find(|(_, m, p)| *m == method && *p == path)
        .map(|(api, ..)| *api)
}

pub fn allowed_route(method: &str, path: &str) -> bool {
    route_api(method, path).is_some()
}

/// The only query string an API's routes accept on the local side; it is not forwarded.
/// Claude Code sends `?beta=true` with every Messages request.
pub fn accepted_query(api: Api) -> Option<&'static str> {
    match api {
        Api::OpenAi => None,
        Api::Anthropic => Some("beta=true"),
    }
}

/// The API version the host requests from Anthropic-compatible backends.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Explicit extension routes; they never grant access to arbitrary backend routes.
pub fn management_route(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET", "/v1/link/models") | ("POST", "/v1/link/model")
    )
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelCommand {
    Load { model: String },
    Unload {},
}

pub fn valid_model_id(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 64
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn control_body_valid(method: &str, body: &[u8]) -> bool {
    if method == "GET" {
        return body.is_empty();
    }
    if body.len() > MAX_CONTROL_BODY_BYTES {
        return false;
    }
    match serde_json::from_slice::<ModelCommand>(body) {
        Ok(ModelCommand::Load { model }) => valid_model_id(&model),
        Ok(ModelCommand::Unload {}) => true,
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointRole {
    Host,
    Client,
}
impl EndpointRole {
    pub fn opposite(self) -> Self {
        match self {
            Self::Host => Self::Client,
            Self::Client => Self::Host,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalContext {
    pub room: String,
    pub session: String,
    pub kind: String,
    pub issued_at: u64,
}

/// Only ciphertext is serializable onto the signaling wire. SDP is never a field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    pub version: u8,
    pub context: SignalContext,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SignalPlaintext {
    sdp: String,
    endpoint: EndpointRole,
}

impl Signal {
    fn associated_data(&self) -> Vec<u8> {
        serde_json::to_vec(&("k-lani-link/sealed-sdp/v2", self.version, &self.context))
            .expect("serializable metadata")
    }
    pub fn seal(
        context: SignalContext,
        sdp: &str,
        endpoint: EndpointRole,
        key: &str,
        nonce: &[u8],
    ) -> Result<Self, String> {
        if nonce.len() != 24
            || sdp.len() > MAX_SDP_BYTES
            || !valid_room(&context.room)
            || !valid_id(&context.session)
            || !matches!(context.kind.as_str(), "offer" | "answer")
        {
            return Err("invalid signaling input".into());
        }
        let plaintext = serde_json::to_vec(&SignalPlaintext {
            sdp: sdp.into(),
            endpoint,
        })
        .map_err(|e| e.to_string())?;
        if plaintext.len() > 24 * 1024 {
            return Err("SDP envelope too large".into());
        }
        let cipher = XChaCha20Poly1305::new_from_slice(&pair_key(key)?)
            .map_err(|_| "invalid connection code")?;
        let mut envelope = Self {
            version: VERSION,
            context,
            nonce: STANDARD.encode(nonce),
            ciphertext: String::new(),
        };
        let nonce_value = XNonce::try_from(nonce).map_err(|_| "invalid nonce")?;
        let encrypted = cipher
            .encrypt(
                &nonce_value,
                Payload {
                    msg: &plaintext,
                    aad: &envelope.associated_data(),
                },
            )
            .map_err(|_| "signaling encryption failed")?;
        envelope.ciphertext = STANDARD.encode(encrypted);
        Ok(envelope)
    }
    pub fn open(
        &self,
        key: &str,
        room: &str,
        kind: &str,
        now: u64,
        expected: EndpointRole,
    ) -> Result<String, String> {
        let c = &self.context;
        if self.version != VERSION
            || c.room != room
            || c.kind != kind
            || !valid_room(room)
            || !valid_id(&c.session)
            || c.issued_at > now.saturating_add(30)
            || now.saturating_sub(c.issued_at) > 120
            || self.ciphertext.len() > 34 * 1024
        {
            return Err("invalid or expired signaling envelope".into());
        }
        let nonce = STANDARD.decode(&self.nonce).map_err(|_| "invalid nonce")?;
        if nonce.len() != 24 {
            return Err("invalid nonce".into());
        }
        let encrypted = STANDARD
            .decode(&self.ciphertext)
            .map_err(|_| "invalid ciphertext")?;
        let cipher = XChaCha20Poly1305::new_from_slice(&pair_key(key)?)
            .map_err(|_| "invalid connection code")?;
        let nonce_value = XNonce::try_from(nonce.as_slice()).map_err(|_| "invalid nonce")?;
        let plaintext = cipher
            .decrypt(
                &nonce_value,
                Payload {
                    msg: &encrypted,
                    aad: &self.associated_data(),
                },
            )
            .map_err(|_| "peer authentication failed")?;
        let value: SignalPlaintext =
            serde_json::from_slice(&plaintext).map_err(|_| "invalid encrypted payload")?;
        if value.endpoint != expected || value.sdp.len() > MAX_SDP_BYTES {
            return Err("wrong peer role or SDP size".into());
        }
        Ok(value.sdp)
    }
}

pub fn valid_room(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

pub fn pair_key(encoded: &str) -> Result<Vec<u8>, String> {
    if encoded.len() != 43 {
        return Err("connection code must be 43 URL-safe base64 characters".into());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| "connection code must be URL-safe base64")?;
    if bytes.len() != 32 {
        return Err("pair_key must contain 32 random bytes".into());
    }
    Ok(bytes)
}

pub fn room_from_key(key: &str) -> Result<String, String> {
    pair_key(key)?;
    Ok(key[..12].to_owned())
}

pub fn valid_id(value: &str) -> bool {
    (16..=64).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn validate_frame_json(json: &str) -> Result<(), wasm_bindgen::JsValue> {
    decode_frame(json.as_bytes())
        .map(|_| ())
        .map_err(|e| wasm_bindgen::JsValue::from_str(&e))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn seal_signal_json(
    context_json: &str,
    sdp: &str,
    key: &str,
    nonce: &[u8],
    host: bool,
) -> Result<String, wasm_bindgen::JsValue> {
    let work = || -> Result<String, String> {
        let context = serde_json::from_str(context_json).map_err(|e| e.to_string())?;
        let role = if host {
            EndpointRole::Host
        } else {
            EndpointRole::Client
        };
        let signal = Signal::seal(context, sdp, role, key, nonce)?;
        serde_json::to_string(&signal).map_err(|e| e.to_string())
    };
    work().map_err(|e| wasm_bindgen::JsValue::from_str(&e))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn open_signal_json(
    json: &str,
    key: &str,
    room: &str,
    kind: &str,
    now: u64,
    expected_host: bool,
) -> Result<String, wasm_bindgen::JsValue> {
    let work = || -> Result<String, String> {
        let signal: Signal = serde_json::from_str(json).map_err(|e| e.to_string())?;
        signal.open(
            key,
            room,
            kind,
            now,
            if expected_host {
                EndpointRole::Host
            } else {
                EndpointRole::Client
            },
        )
    };
    work().map_err(|e| wasm_bindgen::JsValue::from_str(&e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signaling_is_encrypted_and_binds_code_room_roles_session_and_expiry() {
        let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([42; 32]);
        let context = SignalContext {
            room: "office_home".into(),
            session: "session0123456789".into(),
            kind: "offer".into(),
            issued_at: 1000,
        };
        let sdp = "a=fingerprint:sha-256 PRIVATE-FINGERPRINT\r\na=candidate:192.168.1.23";
        let sealed =
            Signal::seal(context.clone(), sdp, EndpointRole::Client, &key, &[7; 24]).unwrap();
        let wire = serde_json::to_string(&sealed).unwrap();
        assert!(
            !wire.contains("PRIVATE-FINGERPRINT")
                && !wire.contains("192.168.1.23")
                && !wire.contains("sdp")
        );
        assert_eq!(
            sealed
                .open(&key, &context.room, "offer", 1010, EndpointRole::Client)
                .unwrap(),
            sdp
        );
        assert!(
            sealed
                .open(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([43; 32]),
                    &context.room,
                    "offer",
                    1010,
                    EndpointRole::Client
                )
                .is_err()
        );
        assert!(
            sealed
                .open(&key, "wrong-room", "offer", 1010, EndpointRole::Client)
                .is_err()
        );
        assert!(
            sealed
                .open(&key, &context.room, "answer", 1010, EndpointRole::Client)
                .is_err()
        );
        assert!(
            sealed
                .open(&key, &context.room, "offer", 1010, EndpointRole::Host)
                .is_err()
        );
        assert!(
            sealed
                .open(&key, &context.room, "offer", 1121, EndpointRole::Client)
                .is_err()
        );
        let mut changed = sealed.clone();
        changed.context.session.push('x');
        assert!(
            changed
                .open(&key, &context.room, "offer", 1010, EndpointRole::Client)
                .is_err()
        );
        changed = sealed.clone();
        changed.version = 1;
        assert!(
            changed
                .open(&key, &context.room, "offer", 1010, EndpointRole::Client)
                .is_err()
        );
        let another = Signal::seal(context, sdp, EndpointRole::Client, &key, &[8; 24]).unwrap();
        assert_ne!(another.ciphertext, sealed.ciphertext);
    }
    #[test]
    fn chunks_preserve_arbitrary_bytes_and_reject_oversize() {
        let bytes: Vec<_> = (0..CHUNK_BYTES).map(|i| i as u8).collect();
        let wire = serde_json::to_vec(&Frame::data(7, &bytes)).unwrap();
        assert!(wire.len() < MAX_FRAME_BYTES);
        let Frame::Data { data, .. } = decode_frame(&wire).unwrap() else {
            panic!()
        };
        assert_eq!(decode_data(&data).unwrap(), bytes);
        assert!(decode_data(&STANDARD.encode(vec![0; CHUNK_BYTES + 1])).is_err());
    }
    #[test]
    fn only_explicit_inference_routes_are_allowed() {
        for (method, path, api) in [
            ("GET", "/v1/models", Api::OpenAi),
            ("POST", "/v1/chat/completions", Api::OpenAi),
            ("POST", "/v1/responses", Api::OpenAi),
            ("POST", "/v1/embeddings", Api::OpenAi),
            ("POST", "/v1/messages", Api::Anthropic),
            ("POST", "/v1/messages/count_tokens", Api::Anthropic),
        ] {
            assert_eq!(route_api(method, path), Some(api), "{method} {path}");
        }
        assert_eq!(ROUTES.len(), 6);
        assert!(!allowed_route("GET", "/v1/messages"));
        assert!(!allowed_route("POST", "/v1/messages?beta=true"));
        assert_eq!(accepted_query(Api::Anthropic), Some("beta=true"));
        assert_eq!(accepted_query(Api::OpenAi), None);
        for path in [
            "http://localhost/admin",
            "/v1/../load",
            "/v1/load",
            "/v1/unload",
            "/v1/vram",
            "/api/models/switch",
            "/api/models/settings",
            "/api/context",
            "/settings",
            "/metrics",
            "/v1/models?x=1",
        ] {
            assert!(!allowed_route("POST", path));
            assert!(!allowed_route("GET", path));
        }
    }
}

#[cfg(test)]
mod management_tests {
    use super::*;
    #[test]
    fn model_control_has_explicit_routes_and_no_paths_or_extra_parameters() {
        assert!(management_route("GET", "/v1/link/models"));
        assert!(management_route("POST", "/v1/link/model"));
        assert!(!allowed_route("POST", "/v1/link/model"));
        assert!(!management_route("POST", "/v1/load"));
        assert!(!management_route("POST", "/v1/link/model?x=1"));
        assert!(control_body_valid(
            "POST",
            br#"{"action":"load","model":"coder-1"}"#
        ));
        assert!(control_body_valid("POST", br#"{"action":"unload"}"#));
        for body in [
            br#"{}"#.as_slice(),
            br#"{"action":"load","model":"../file"}"#,
            br#"{"action":"load","model":"coder","args":"--debug"}"#,
            br#"{"action":"unload","model":"coder"}"#,
        ] {
            assert!(!control_body_valid("POST", body));
        }
        assert!(!control_body_valid("GET", b"{}"));
    }
}
