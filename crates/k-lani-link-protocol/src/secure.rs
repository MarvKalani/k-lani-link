//! Authenticated application records above DTLS, with directional keys and strict counters.
use super::{
    EndpointRole, Frame, MAX_FRAME_BYTES, Signal, decode_batch, decode_frame, pair_key,
    room_from_key,
};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub const MAX_PACKET_BYTES: usize = MAX_FRAME_BYTES + 12 + 16;
/// Upper bound of frames in one batched record; every frame still needs about a dozen bytes.
pub const MAX_BATCH_FRAMES: usize = 256;
const MAGIC: &[u8; 4] = b"KLI2";
/// Plaintext sizes every record is padded to, so record sizes reveal only a coarse bucket,
/// not the length of a token, prompt or tool call. The padding is ASCII spaces after the
/// JSON, which JSON allows: receivers, including older ones, decode padded records
/// unchanged. Counts and timing of records remain visible.
pub const PADDED_SIZES: [usize; 5] = [1024, 2048, 4096, 8192, MAX_FRAME_BYTES];

struct Direction {
    cipher: XChaCha20Poly1305,
    sequence: u64,
}

pub struct SecureSession {
    send: Direction,
    receive: Direction,
}

impl SecureSession {
    pub fn new(
        code: &str,
        offer: &Signal,
        answer: &Signal,
        local: EndpointRole,
        offer_owner: EndpointRole,
        now: u64,
    ) -> Result<Self, String> {
        let room = room_from_key(code)?;
        offer.open(code, &room, "offer", now, offer_owner)?;
        answer.open(code, &room, "answer", now, offer_owner.opposite())?;
        if offer.context.session != answer.context.session {
            return Err("session mismatch".into());
        }
        // Both fresh random envelope nonces participate, including on a repeated offer.
        let transcript = serde_json::to_vec(&("k-lani-link/session/v2", offer, answer))
            .map_err(|e| e.to_string())?;
        let salt = Sha256::digest(transcript);
        let kdf = Hkdf::<Sha256>::new(Some(&salt), &pair_key(code)?);
        let derive = |label: &[u8]| -> Result<Direction, String> {
            let mut key = [0u8; 32];
            kdf.expand(label, &mut key)
                .map_err(|_| "key derivation failed")?;
            let cipher =
                XChaCha20Poly1305::new_from_slice(&key).map_err(|_| "invalid session key")?;
            key.zeroize();
            Ok(Direction {
                cipher,
                sequence: 0,
            })
        };
        let client_to_host = derive(b"k-lani-link/payload/v2/client-to-host")?;
        let host_to_client = derive(b"k-lani-link/payload/v2/host-to-client")?;
        Ok(match local {
            EndpointRole::Client => Self {
                send: client_to_host,
                receive: host_to_client,
            },
            EndpointRole::Host => Self {
                send: host_to_client,
                receive: client_to_host,
            },
        })
    }

    /// Direction keys that were agreed some other way, for example by the mailbox hello.
    pub(crate) fn from_keys(
        client_to_host: &[u8; 32],
        host_to_client: &[u8; 32],
        local: EndpointRole,
    ) -> Result<Self, String> {
        let direction = |key: &[u8; 32]| -> Result<Direction, String> {
            Ok(Direction {
                cipher: XChaCha20Poly1305::new_from_slice(key)
                    .map_err(|_| "invalid session key")?,
                sequence: 0,
            })
        };
        let (client_to_host, host_to_client) =
            (direction(client_to_host)?, direction(host_to_client)?);
        Ok(match local {
            EndpointRole::Client => Self {
                send: client_to_host,
                receive: host_to_client,
            },
            EndpointRole::Host => Self {
                send: host_to_client,
                receive: client_to_host,
            },
        })
    }

    /// The counter the next sealed record will carry (the mailbox addresses records by it).
    pub fn send_sequence(&self) -> u64 {
        self.send.sequence
    }

    /// The counter the next opened record must carry.
    pub fn receive_sequence(&self) -> u64 {
        self.receive.sequence
    }

    pub fn seal(&mut self, frame: &Frame) -> Result<Vec<u8>, String> {
        self.seal_plaintext(&serde_json::to_vec(frame).map_err(|e| e.to_string())?)
    }

    /// Several frames in one record, for transports that pay per request.
    pub fn seal_batch(&mut self, frames: &[Frame]) -> Result<Vec<u8>, String> {
        if frames.is_empty() || frames.len() > MAX_BATCH_FRAMES {
            return Err("invalid frame batch".into());
        }
        self.seal_plaintext(&serde_json::to_vec(frames).map_err(|e| e.to_string())?)
    }

    pub fn open(&mut self, packet: &[u8]) -> Result<Frame, String> {
        self.open_with(packet, decode_frame)
    }

    pub fn open_batch(&mut self, packet: &[u8]) -> Result<Vec<Frame>, String> {
        self.open_with(packet, decode_batch)
    }

    fn seal_plaintext(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, String> {
        if plaintext.len() > MAX_FRAME_BYTES {
            return Err("frame too large".into());
        }
        let size = PADDED_SIZES
            .into_iter()
            .find(|size| *size >= plaintext.len())
            .unwrap_or(MAX_FRAME_BYTES);
        let mut padded = Vec::with_capacity(size);
        padded.extend_from_slice(plaintext);
        padded.resize(size, b' ');
        let next = self
            .send
            .sequence
            .checked_add(1)
            .ok_or("sequence exhausted")?;
        let mut header = Vec::from(MAGIC.as_slice());
        header.extend_from_slice(&self.send.sequence.to_be_bytes());
        let nonce = nonce(self.send.sequence);
        let ciphertext = self
            .send
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &padded,
                    aad: &header,
                },
            )
            .map_err(|_| "record encryption failed")?;
        self.send.sequence = next;
        header.extend_from_slice(&ciphertext);
        Ok(header)
    }

    /// The counter advances only once the plaintext also decoded.
    fn open_with<T>(
        &mut self,
        packet: &[u8],
        decode: impl FnOnce(&[u8]) -> Result<T, String>,
    ) -> Result<T, String> {
        if !(28..=MAX_PACKET_BYTES).contains(&packet.len()) || &packet[..4] != MAGIC {
            return Err("invalid encrypted record".into());
        }
        let sequence = u64::from_be_bytes(packet[4..12].try_into().map_err(|_| "invalid counter")?);
        if sequence != self.receive.sequence {
            return Err("repeated or out-of-order record".into());
        }
        let next = sequence.checked_add(1).ok_or("sequence exhausted")?;
        let plaintext = self
            .receive
            .cipher
            .decrypt(
                &nonce(sequence),
                Payload {
                    msg: &packet[12..],
                    aad: &packet[..12],
                },
            )
            .map_err(|_| "record authentication failed")?;
        let decoded = decode(&plaintext)?;
        self.receive.sequence = next;
        Ok(decoded)
    }
}

fn nonce(sequence: u64) -> XNonce {
    let mut nonce = [0u8; 24];
    nonce[16..].copy_from_slice(&sequence.to_be_bytes());
    nonce.into()
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub struct WasmSecureSession {
    inner: SecureSession,
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
impl WasmSecureSession {
    #[wasm_bindgen::prelude::wasm_bindgen(constructor)]
    pub fn new(
        code: &str,
        offer_json: &str,
        answer_json: &str,
        local_host: bool,
        offer_host: bool,
        now: u64,
    ) -> Result<WasmSecureSession, wasm_bindgen::JsValue> {
        let work = || -> Result<Self, String> {
            let offer = serde_json::from_str(offer_json).map_err(|e| e.to_string())?;
            let answer = serde_json::from_str(answer_json).map_err(|e| e.to_string())?;
            let role = |host| {
                if host {
                    EndpointRole::Host
                } else {
                    EndpointRole::Client
                }
            };
            Ok(Self {
                inner: SecureSession::new(
                    code,
                    &offer,
                    &answer,
                    role(local_host),
                    role(offer_host),
                    now,
                )?,
            })
        };
        work().map_err(|e| wasm_bindgen::JsValue::from_str(&e))
    }
    pub fn encrypt_frame(&mut self, json: &str) -> Result<Vec<u8>, wasm_bindgen::JsValue> {
        decode_frame(json.as_bytes())
            .and_then(|frame| self.inner.seal(&frame))
            .map_err(|e| wasm_bindgen::JsValue::from_str(&e))
    }
    pub fn decrypt_frame(&mut self, packet: &[u8]) -> Result<String, wasm_bindgen::JsValue> {
        self.inner
            .open(packet)
            .and_then(|frame| serde_json::to_string(&frame).map_err(|e| e.to_string()))
            .map_err(|e| wasm_bindgen::JsValue::from_str(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SignalContext;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

    fn peers(answer_nonce: u8) -> (SecureSession, SecureSession) {
        let code = URL_SAFE_NO_PAD.encode([42; 32]);
        let context = |kind: &str| SignalContext {
            room: room_from_key(&code).unwrap(),
            session: "session0123456789".into(),
            kind: kind.into(),
            issued_at: 1000,
        };
        let offer = Signal::seal(
            context("offer"),
            "private offer",
            EndpointRole::Client,
            &code,
            &[1; 24],
        )
        .unwrap();
        let answer = Signal::seal(
            context("answer"),
            "private answer",
            EndpointRole::Host,
            &code,
            &[answer_nonce; 24],
        )
        .unwrap();
        (
            SecureSession::new(
                &code,
                &offer,
                &answer,
                EndpointRole::Client,
                EndpointRole::Client,
                1010,
            )
            .unwrap(),
            SecureSession::new(
                &code,
                &offer,
                &answer,
                EndpointRole::Host,
                EndpointRole::Client,
                1010,
            )
            .unwrap(),
        )
    }
    #[test]
    fn rejects_tampering_replay_reflection_and_plaintext() {
        let (mut client, mut host) = peers(2);
        let packet = client
            .seal(&Frame::Request {
                id: 1,
                method: "POST".into(),
                path: "private-sensitive-marker".into(),
            })
            .unwrap();
        assert!(!String::from_utf8_lossy(&packet).contains("private-sensitive-marker"));
        let mut corrupt = packet.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert!(host.open(&corrupt).is_err());
        assert!(client.open(&packet).is_err());
        assert!(host.open(br#"{"type":"ready"}"#).is_err());
        assert!(matches!(
            host.open(&packet).unwrap(),
            Frame::Request { id: 1, .. }
        ));
        assert!(host.open(&packet).is_err());
        let response = host.seal(&Frame::End { id: 1 }).unwrap();
        assert!(matches!(
            client.open(&response).unwrap(),
            Frame::End { id: 1 }
        ));
    }
    #[test]
    fn records_reveal_only_their_padding_bucket_and_still_decode() {
        let (mut client, mut host) = peers(2);
        let overhead = MAX_PACKET_BYTES - MAX_FRAME_BYTES;
        let json = |frames: &[Frame]| serde_json::to_string(frames).unwrap();
        // Tokens of different lengths, a credit and a full chunk.
        let small: Vec<Frame> = ["a", "a longer token", &"x".repeat(300)]
            .iter()
            .map(|token| Frame::data(7, token.as_bytes()))
            .chain([Frame::Credit { id: 7 }, Frame::Ready])
            .collect();
        for frame in &small {
            let packet = host.seal(frame).unwrap();
            assert_eq!(packet.len(), PADDED_SIZES[0] + overhead);
            assert_eq!(
                json(&[client.open(&packet).unwrap()]),
                json(std::slice::from_ref(frame))
            );
        }
        let full = Frame::data(7, &[0xff; crate::CHUNK_BYTES]);
        let packet = host.seal(&full).unwrap();
        assert_eq!(packet.len(), MAX_PACKET_BYTES);
        assert_eq!(json(&[client.open(&packet).unwrap()]), json(&[full]));
        let batch = host.seal_batch(&small).unwrap();
        assert!(
            PADDED_SIZES
                .iter()
                .any(|size| batch.len() == size + overhead)
        );
        assert_eq!(json(&client.open_batch(&batch).unwrap()), json(&small));
        // What an older receiver sees after decryption is still valid JSON.
        let mut plain = serde_json::to_vec(&small[1]).unwrap();
        plain.resize(PADDED_SIZES[0], b' ');
        assert_eq!(json(&[decode_frame(&plain).unwrap()]), json(&small[1..2]));
    }
    #[test]
    fn fresh_handshake_changes_keys_even_for_the_same_offer() {
        let (mut first, _) = peers(2);
        let (mut second, _) = peers(3);
        assert_ne!(
            first.seal(&Frame::Ready).unwrap(),
            second.seal(&Frame::Ready).unwrap()
        );
    }
}
