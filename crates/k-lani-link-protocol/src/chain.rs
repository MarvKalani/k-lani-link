//! The key chain and the sealed hello of the mailbox transport (docs/DESIGN-MAILBOX.md).
//!
//! No I/O and no clock or randomness of its own: callers pass `now`, the ephemeral secret and the
//! nonce, as `Signal::seal` does. Persisting a state *before* the step that depends on it is the
//! caller's job; `HostChains` only says what the next state is.
use super::{EndpointRole, Frame, SecureSession, pair_key};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use x25519_dalek::{X25519_BASEPOINT_BYTES, x25519};
use zeroize::Zeroize;

const MAGIC: &[u8; 4] = b"KLH1";
const ROLE_CLIENT: u8 = 1;
const ROLE_HOST: u8 = 2;
const STATUS_OK: u8 = 0;
const STATUS_STALE: u8 = 1;
/// Same tolerance as signaling envelopes: a hello is only good for a short moment.
const MAX_AGE_SECONDS: u64 = 120;
const MAX_FUTURE_SECONDS: u64 = 30;
const HELLO_PLAINTEXT: usize = 16 + 32 + 8;
const REPLY_PLAINTEXT: usize = 1 + HELLO_PLAINTEXT;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;

/// A hello or a reply is fixed size; the box learns nothing from its length.
pub const HELLO_BYTES: usize = 4 + 1 + NONCE_BYTES + HELLO_PLAINTEXT + TAG_BYTES;
pub const REPLY_BYTES: usize = 4 + 1 + NONCE_BYTES + REPLY_PLAINTEXT + TAG_BYTES;

/// Why a hello or reply was not accepted. Fixed texts only, never data of the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloError {
    /// The peer proved it holds an older state of this pair: copied, or out of sync.
    Stale,
    Invalid,
    PendingFull,
}
impl std::fmt::Display for HelloError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stale => "pairing state is out of date, possibly copied; create a new pair",
            Self::Invalid => "invalid or expired hello",
            Self::PendingFull => {
                "pending handshake capacity reached; use a saved successor or create a new pair"
            }
        })
    }
}
impl std::error::Error for HelloError {}

/// One link of the chain: 32 secret bytes. `chain_0` comes from the code you carried over.
#[derive(Clone)]
pub struct Chain([u8; 32]);

impl Drop for Chain {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Chain {
    /// The first link, separated from every other use of the same code.
    pub fn from_code(code: &str) -> Result<Self, String> {
        let kdf = Hkdf::<Sha256>::new(None, &pair_key(code)?);
        let mut key = [0u8; 32];
        kdf.expand(b"k-lani-link/mailbox/chain/v1/initial", &mut key)
            .map_err(|_| "key derivation failed")?;
        Ok(Self(key))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// For the state file, which is a secret like the code itself.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    fn expand(&self, label: &[u8]) -> [u8; 32] {
        let kdf = Hkdf::<Sha256>::from_prk(&self.0).expect("32 bytes are a valid PRK");
        let mut out = [0u8; 32];
        kdf.expand(label, &mut out).expect("32 bytes fit");
        out
    }

    fn room_id(&self) -> [u8; 16] {
        let mut id = [0u8; 16];
        id.copy_from_slice(&self.expand(b"k-lani-link/mailbox/room/v1")[..16]);
        id
    }

    /// Where this link's hello and records live on the box. Reveals nothing about the key.
    pub fn room(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.room_id())
    }

    fn hello_key(&self, role: u8) -> [u8; 32] {
        self.expand(if role == ROLE_CLIENT {
            b"k-lani-link/mailbox/hello/v1/client"
        } else {
            b"k-lani-link/mailbox/hello/v1/host"
        })
    }

    /// What a host keeps of a link it left behind: enough to recognise a stale peer, not enough
    /// to open a session or to compute any later link.
    pub fn retire(&self) -> Retired {
        Retired {
            room_id: self.room_id(),
            client_key: self.hello_key(ROLE_CLIENT),
            host_key: self.hello_key(ROLE_HOST),
        }
    }
}

#[derive(Clone)]
pub struct Retired {
    room_id: [u8; 16],
    client_key: [u8; 32],
    host_key: [u8; 32],
}

impl Drop for Retired {
    fn drop(&mut self) {
        self.client_key.zeroize();
        self.host_key.zeroize();
    }
}

impl Retired {
    pub fn room(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.room_id)
    }
    pub fn to_bytes(&self) -> [u8; 80] {
        let mut out = [0u8; 80];
        out[..16].copy_from_slice(&self.room_id);
        out[16..48].copy_from_slice(&self.client_key);
        out[48..].copy_from_slice(&self.host_key);
        out
    }
    pub fn from_bytes(bytes: &[u8; 80]) -> Self {
        Self {
            room_id: bytes[..16].try_into().expect("16 bytes"),
            client_key: bytes[16..48].try_into().expect("32 bytes"),
            host_key: bytes[48..].try_into().expect("32 bytes"),
        }
    }
}

/// A finished handshake: the record session and the link that follows this one.
pub struct Established {
    pub session: SecureSession,
    next: Chain,
    source_room: String,
    session_id: String,
}
impl Established {
    pub fn next(&self) -> &Chain {
        &self.next
    }
    pub fn source_room(&self) -> &str {
        &self.source_room
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

fn fresh(issued_at: u64, now: u64) -> bool {
    issued_at <= now.saturating_add(MAX_FUTURE_SECONDS)
        && now.saturating_sub(issued_at) <= MAX_AGE_SECONDS
}

fn seal(
    key: &[u8; 32],
    role: u8,
    room_id: &[u8; 16],
    bound: &[u8],
    nonce: &[u8; NONCE_BYTES],
    plaintext: &[u8],
) -> Result<Vec<u8>, HelloError> {
    let mut out = [MAGIC.as_slice(), &[role], nonce].concat();
    let aad = [MAGIC.as_slice(), &[role], room_id, bound].concat();
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| HelloError::Invalid)?;
    let sealed = cipher
        .encrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| HelloError::Invalid)?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn open(
    key: &[u8; 32],
    role: u8,
    room_id: &[u8; 16],
    bound: &[u8],
    wire: &[u8],
    plaintext_len: usize,
) -> Result<Vec<u8>, HelloError> {
    if wire.len() != 4 + 1 + NONCE_BYTES + plaintext_len + TAG_BYTES
        || &wire[..4] != MAGIC
        || wire[4] != role
    {
        return Err(HelloError::Invalid);
    }
    let aad = [MAGIC.as_slice(), &[role], room_id, bound].concat();
    let nonce: [u8; NONCE_BYTES] = wire[5..5 + NONCE_BYTES].try_into().expect("24 bytes");
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| HelloError::Invalid)?
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &wire[5 + NONCE_BYTES..],
                aad: &aad,
            },
        )
        .map_err(|_| HelloError::Invalid)
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    x25519(*secret, X25519_BASEPOINT_BYTES)
}

fn transcript_hash(hello: &[u8], reply: &[u8]) -> [u8; 32] {
    let mut transcript = Sha256::new();
    transcript.update(b"k-lani-link/mailbox/transcript/v1");
    for part in [hello, reply] {
        transcript.update((part.len() as u32).to_be_bytes());
        transcript.update(part);
    }
    transcript.finalize().into()
}

fn keys(
    chain: &Chain,
    secret: &[u8; 32],
    peer: &[u8; 32],
    hello: &[u8],
    reply: &[u8],
    local: EndpointRole,
) -> Result<Established, HelloError> {
    let mut shared = x25519(*secret, *peer);
    // A low-order peer key would make the shared secret independent of our secret.
    if shared == [0u8; 32] {
        return Err(HelloError::Invalid);
    }
    let transcript = transcript_hash(hello, reply);
    let mut ikm = [chain.0.as_slice(), shared.as_slice()].concat();
    shared.zeroize();
    let kdf = Hkdf::<Sha256>::new(Some(&transcript), &ikm);
    ikm.zeroize();
    let derive = |label: &[u8]| {
        let mut key = [0u8; 32];
        kdf.expand(label, &mut key).expect("32 bytes fit");
        key
    };
    let (mut to_host, mut to_client, mut next) = (
        derive(b"k-lani-link/mailbox/payload/v1/client-to-host"),
        derive(b"k-lani-link/mailbox/payload/v1/host-to-client"),
        derive(b"k-lani-link/mailbox/chain/v1/next"),
    );
    let session =
        SecureSession::from_keys(&to_host, &to_client, local).map_err(|_| HelloError::Invalid);
    to_host.zeroize();
    to_client.zeroize();
    let established = session.map(|session| Established {
        session,
        next: Chain(next),
        source_room: chain.room(),
        session_id: URL_SAFE_NO_PAD.encode(transcript),
    });
    next.zeroize();
    established
}

/// The client side of one attempt. Keep it until the reply arrives.
pub struct Initiator {
    chain: Chain,
    secret: [u8; 32],
    session: [u8; 16],
    hello: Vec<u8>,
}

impl Drop for Initiator {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl Initiator {
    pub fn start(
        chain: &Chain,
        secret: [u8; 32],
        session: [u8; 16],
        nonce: [u8; NONCE_BYTES],
        now: u64,
    ) -> Result<Self, HelloError> {
        let plaintext = [session.as_slice(), &public_key(&secret), &now.to_be_bytes()].concat();
        let hello = seal(
            &chain.hello_key(ROLE_CLIENT),
            ROLE_CLIENT,
            &chain.room_id(),
            &[],
            &nonce,
            &plaintext,
        )?;
        Ok(Self {
            chain: chain.clone(),
            secret,
            session,
            hello,
        })
    }

    pub fn room(&self) -> String {
        self.chain.room()
    }

    pub fn hello(&self) -> &[u8] {
        &self.hello
    }

    pub fn finish(self, reply: &[u8], now: u64) -> Result<Established, HelloError> {
        let plain = open(
            &self.chain.hello_key(ROLE_HOST),
            ROLE_HOST,
            &self.chain.room_id(),
            &hash(&self.hello),
            reply,
            REPLY_PLAINTEXT,
        )?;
        let issued_at = u64::from_be_bytes(plain[49..57].try_into().expect("8 bytes"));
        if plain[1..17] != self.session || !fresh(issued_at, now) {
            return Err(HelloError::Invalid);
        }
        match plain[0] {
            STATUS_STALE => Err(HelloError::Stale),
            STATUS_OK => keys(
                &self.chain,
                &self.secret,
                plain[17..49].try_into().expect("32 bytes"),
                &self.hello,
                reply,
                EndpointRole::Client,
            ),
            _ => Err(HelloError::Invalid),
        }
    }
}

/// Which of the host's links opened the hello.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The client holds the link the host has confirmed.
    Current,
    /// The client holds the successor of the last session, so it did store it.
    Pending,
}

pub struct Accepted {
    confirmation_available: bool,
    reply: Vec<u8>,
    via: Via,
    pub session: SecureSession,
    next: Chain,
    source_room: String,
    hello_hash: [u8; 32],
    session_id: String,
}
impl Accepted {
    pub fn reply(&self) -> &[u8] {
        &self.reply
    }
    pub fn next(&self) -> &Chain {
        &self.next
    }
    pub fn via(&self) -> Via {
        self.via
    }
    pub fn source_room(&self) -> &str {
        &self.source_room
    }
    pub fn hello_hash(&self) -> &[u8; 32] {
        &self.hello_hash
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

pub enum Verdict {
    Accepted(Accepted),
    /// Byte-identical saved reply. Deliberately contains no cipher or resettable counters.
    CachedReply(Vec<u8>),
    /// A known confirmed hello replay, not evidence of a newly active stale peer.
    Ignored,
    Stale(Vec<u8>),
}

pub const MAX_PENDING: usize = 4;
pub const MAX_RETIRED: usize = 3;
pub const PENDING_BYTES: usize = 32 + REPLY_BYTES + 32;
pub const RETIRED_GENERATION_BYTES: usize = 80 + 32;

#[derive(Clone)]
pub struct Pending {
    hello_hash: [u8; 32],
    reply: Vec<u8>,
    next: Chain,
}
impl Pending {
    pub fn hello_hash(&self) -> &[u8; 32] {
        &self.hello_hash
    }
    pub fn next(&self) -> &Chain {
        &self.next
    }
    pub fn reply(&self) -> &[u8] {
        &self.reply
    }
    pub fn to_bytes(&self) -> [u8; PENDING_BYTES] {
        let mut out = [0; PENDING_BYTES];
        out[..32].copy_from_slice(&self.hello_hash);
        out[32..32 + REPLY_BYTES].copy_from_slice(&self.reply);
        out[32 + REPLY_BYTES..].copy_from_slice(self.next.as_bytes());
        out
    }
    pub fn from_bytes(bytes: &[u8; PENDING_BYTES]) -> Self {
        Self {
            hello_hash: bytes[..32].try_into().unwrap(),
            reply: bytes[32..32 + REPLY_BYTES].to_vec(),
            next: Chain::from_bytes(bytes[32 + REPLY_BYTES..].try_into().unwrap()),
        }
    }
}

#[derive(Clone)]
pub struct RetiredGeneration {
    keys: Retired,
    confirmed_hello: [u8; 32],
}
impl RetiredGeneration {
    pub fn room(&self) -> String {
        self.keys.room()
    }
    pub fn confirmed_hello(&self) -> &[u8; 32] {
        &self.confirmed_hello
    }
    pub fn to_bytes(&self) -> [u8; RETIRED_GENERATION_BYTES] {
        let mut out = [0; RETIRED_GENERATION_BYTES];
        out[..80].copy_from_slice(&self.keys.to_bytes());
        out[80..].copy_from_slice(&self.confirmed_hello);
        out
    }
    pub fn from_bytes(bytes: &[u8; RETIRED_GENERATION_BYTES]) -> Self {
        Self {
            keys: Retired::from_bytes(bytes[..80].try_into().unwrap()),
            confirmed_hello: bytes[80..].try_into().unwrap(),
        }
    }
}

/// Bounded durable credential state. It never contains a restorable data-session cipher.
#[derive(Clone)]
pub struct HostChains {
    cur: Chain,
    pending: Vec<Pending>,
    retired: Vec<RetiredGeneration>,
}
impl HostChains {
    pub fn new(cur: Chain) -> Self {
        Self {
            cur,
            pending: Vec::new(),
            retired: Vec::new(),
        }
    }
    pub fn current(&self) -> &Chain {
        &self.cur
    }
    pub fn pending(&self) -> &[Pending] {
        &self.pending
    }
    pub fn retired(&self) -> &[RetiredGeneration] {
        &self.retired
    }
    pub fn last_confirmed_hello(&self) -> Option<&[u8; 32]> {
        self.retired.first().map(|r| &r.confirmed_hello)
    }

    /// Restore only a structurally consistent state. The caller must also check file policy/lifetime.
    pub fn restore(
        cur: Chain,
        pending: Vec<Pending>,
        retired: Vec<RetiredGeneration>,
    ) -> Result<Self, HelloError> {
        if pending.len() > MAX_PENDING || retired.len() > MAX_RETIRED {
            return Err(HelloError::Invalid);
        }
        let mut rooms = vec![cur.room()];
        let mut hashes = Vec::new();
        for entry in &pending {
            let plain = open(
                &cur.hello_key(ROLE_HOST),
                ROLE_HOST,
                &cur.room_id(),
                &entry.hello_hash,
                &entry.reply,
                REPLY_PLAINTEXT,
            )?;
            if plain[0] != STATUS_OK
                || rooms.contains(&entry.next.room())
                || hashes.contains(&entry.hello_hash)
            {
                return Err(HelloError::Invalid);
            }
            rooms.push(entry.next.room());
            hashes.push(entry.hello_hash);
        }
        for entry in &retired {
            if rooms.contains(&entry.room()) || hashes.contains(&entry.confirmed_hello) {
                return Err(HelloError::Invalid);
            }
            rooms.push(entry.room());
            hashes.push(entry.confirmed_hello);
        }
        Ok(Self {
            cur,
            pending,
            retired,
        })
    }
    pub fn rooms(&self) -> Vec<String> {
        let mut rooms = vec![self.cur.room()];
        rooms.extend(self.pending.iter().map(|p| p.next.room()));
        rooms.extend(self.retired.iter().map(RetiredGeneration::room));
        rooms
    }
    pub fn accept(
        &self,
        room: &str,
        hello: &[u8],
        secret: [u8; 32],
        nonce: [u8; NONCE_BYTES],
        now: u64,
    ) -> Result<Verdict, HelloError> {
        let hello_hash = hash(hello);
        if room == self.cur.room()
            && let Some(entry) = self.pending.iter().find(|p| p.hello_hash == hello_hash)
        {
            return Ok(Verdict::CachedReply(entry.reply.clone()));
        }
        if self
            .retired
            .iter()
            .any(|r| r.room() == room && r.confirmed_hello == hello_hash)
        {
            return Ok(Verdict::Ignored);
        }
        let source = if self.cur.room() == room {
            Some((Via::Current, &self.cur))
        } else {
            self.pending
                .iter()
                .find(|p| p.next.room() == room)
                .map(|p| (Via::Pending, &p.next))
        };
        if let Some((via, chain)) = source {
            let (challenge, peer) =
                read_hello(&chain.hello_key(ROLE_CLIENT), &chain.room_id(), hello, now)?;
            if via == Via::Current && self.pending.len() == MAX_PENDING {
                return Err(HelloError::PendingFull);
            }
            let mut plain = vec![STATUS_OK];
            plain.extend_from_slice(&challenge);
            plain.extend_from_slice(&public_key(&secret));
            plain.extend_from_slice(&now.to_be_bytes());
            let reply = seal(
                &chain.hello_key(ROLE_HOST),
                ROLE_HOST,
                &chain.room_id(),
                &hello_hash,
                &nonce,
                &plain,
            )?;
            let Established {
                session,
                next,
                source_room,
                session_id,
            } = keys(chain, &secret, &peer, hello, &reply, EndpointRole::Host)?;
            return Ok(Verdict::Accepted(Accepted {
                confirmation_available: true,
                reply,
                via,
                session,
                next,
                source_room,
                hello_hash,
                session_id,
            }));
        }
        if let Some(retired) = self.retired.iter().find(|r| r.room() == room) {
            let keys = &retired.keys;
            let (challenge, _) = read_hello(&keys.client_key, &keys.room_id, hello, now)?;
            let mut plain = vec![STATUS_STALE];
            plain.extend_from_slice(&challenge);
            plain.extend_from_slice(&[0; 32]);
            plain.extend_from_slice(&now.to_be_bytes());
            return Ok(Verdict::Stale(seal(
                &keys.host_key,
                ROLE_HOST,
                &keys.room_id,
                &hello_hash,
                &nonce,
                &plain,
            )?));
        }
        Err(HelloError::Invalid)
    }
    fn retire_current(&mut self, confirmed_hello: [u8; 32]) {
        self.retired.insert(
            0,
            RetiredGeneration {
                keys: self.cur.retire(),
                confirmed_hello,
            },
        );
        self.retired.truncate(MAX_RETIRED);
    }
    /// Store the returned state before exposing Accepted::reply to a peer.
    pub fn after_accept(&self, accepted: &Accepted) -> Result<Self, HelloError> {
        let mut state = self.clone();
        if accepted.source_room == state.cur.room() {
            if let Some(entry) = state
                .pending
                .iter()
                .find(|p| p.hello_hash == accepted.hello_hash)
            {
                return if entry.reply == accepted.reply
                    && entry.next.as_bytes() == accepted.next.as_bytes()
                {
                    Ok(state)
                } else {
                    Err(HelloError::Invalid)
                };
            }
            if state.pending.len() == MAX_PENDING {
                return Err(HelloError::PendingFull);
            }
        } else {
            let predecessor = state
                .pending
                .iter()
                .find(|p| p.next.room() == accepted.source_room)
                .ok_or(HelloError::Invalid)?
                .clone();
            state.retire_current(predecessor.hello_hash);
            state.cur = predecessor.next;
            state.pending.clear();
        }
        state.pending.push(Pending {
            hello_hash: accepted.hello_hash,
            reply: accepted.reply.clone(),
            next: accepted.next.clone(),
        });
        Self::restore(state.cur, state.pending, state.retired)
    }
    /// A confirmation must belong to a still-pending, live attempt. No arbitrary key promotion.
    pub fn confirm(&self, accepted: &mut Accepted, ready: &[u8]) -> Result<Self, HelloError> {
        let available = accepted.confirmation_available;
        accepted.confirmation_available = false;
        if !available
            || accepted.source_room != self.cur.room()
            || !self.pending.iter().any(|p| {
                p.hello_hash == accepted.hello_hash
                    && p.reply == accepted.reply
                    && p.next.as_bytes() == accepted.next.as_bytes()
            })
            || accepted.session.receive_sequence() != 0
            || accepted.session.send_sequence() != 0
        {
            return Err(HelloError::Invalid);
        }
        let frames = accepted
            .session
            .open_batch(ready)
            .map_err(|_| HelloError::Invalid)?;
        if !matches!(frames.as_slice(), [Frame::Ready]) {
            return Err(HelloError::Invalid);
        }
        let mut state = self.clone();
        state.retire_current(accepted.hello_hash);
        state.cur = accepted.next.clone();
        state.pending.clear();
        Ok(state)
    }
}

fn read_hello(
    key: &[u8; 32],
    room_id: &[u8; 16],
    hello: &[u8],
    now: u64,
) -> Result<([u8; 16], [u8; 32]), HelloError> {
    let plain = open(key, ROLE_CLIENT, room_id, &[], hello, HELLO_PLAINTEXT)?;
    let issued_at = u64::from_be_bytes(plain[48..56].try_into().expect("8 bytes"));
    if !fresh(issued_at, now) {
        return Err(HelloError::Invalid);
    }
    Ok((
        plain[..16].try_into().expect("16 bytes"),
        plain[16..48].try_into().expect("32 bytes"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frame, valid_room};

    const NOW: u64 = 1_000_000;

    fn code(byte: u8) -> String {
        URL_SAFE_NO_PAD.encode([byte; 32])
    }

    fn start(chain: &Chain, seed: u8, now: u64) -> Initiator {
        Initiator::start(chain, [seed; 32], [seed; 16], [seed; 24], now).unwrap()
    }

    fn accept(
        host: &HostChains,
        initiator: &Initiator,
        seed: u8,
        now: u64,
    ) -> Result<Verdict, HelloError> {
        host.accept(
            &initiator.room(),
            initiator.hello(),
            [seed; 32],
            [seed; 24],
            now,
        )
    }

    fn accepted(verdict: Result<Verdict, HelloError>) -> Accepted {
        match verdict {
            Ok(Verdict::Accepted(accepted)) => accepted,
            Ok(_) => panic!("expected a new accepted attempt"),
            Err(error) => panic!("unexpectedly {error}"),
        }
    }

    fn ping(from: &mut SecureSession, to: &mut SecureSession) {
        let record = from.seal_batch(&[Frame::Ready]).unwrap();
        assert!(matches!(
            to.open_batch(&record).unwrap()[..],
            [Frame::Ready]
        ));
    }

    #[test]
    fn both_sides_agree_on_records_and_on_the_next_link() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let initiator = start(&chain, 7, NOW);
        let Accepted {
            reply,
            via,
            session: mut host_session,
            next: host_next,
            ..
        } = accepted(accept(&host, &initiator, 9, NOW + 1));
        assert_eq!(via, Via::Current);
        assert_eq!(initiator.hello().len(), HELLO_BYTES);
        assert_eq!(reply.len(), REPLY_BYTES);
        let Established {
            session: mut client_session,
            next: client_next,
            ..
        } = initiator.finish(&reply, NOW + 2).unwrap();
        assert_eq!(host_next.as_bytes(), client_next.as_bytes());
        assert_ne!(host_next.as_bytes(), chain.as_bytes());
        ping(&mut client_session, &mut host_session);
        ping(&mut host_session, &mut client_session);
        assert_eq!(client_session.send_sequence(), 1);
        assert_eq!(client_session.receive_sequence(), 1);
    }

    #[test]
    fn rooms_are_derived_valid_and_do_not_contain_the_code() {
        let first = Chain::from_code(&code(1)).unwrap();
        let second = Chain::from_code(&code(2)).unwrap();
        assert!(valid_room(&first.room()));
        assert_eq!(first.room().len(), 22);
        assert_ne!(first.room(), second.room());
        assert!(!code(1).contains(&first.room()[..8]));
        assert!(!code(1).starts_with(&first.room()[..8]));
        assert_eq!(first.room(), Chain::from_code(&code(1)).unwrap().room());
    }

    #[test]
    fn a_wrong_chain_or_room_is_not_opened() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let other = Chain::from_code(&code(2)).unwrap();
        let initiator = start(&other, 7, NOW);
        let host = HostChains::new(chain.clone());
        // Right room by accident, wrong key: still refused.
        assert_eq!(
            host.accept(&chain.room(), initiator.hello(), [9; 32], [9; 24], NOW)
                .err(),
            Some(HelloError::Invalid)
        );
        assert_eq!(
            accept(&host, &initiator, 9, NOW).err(),
            Some(HelloError::Invalid)
        );
    }

    #[test]
    fn tampering_with_hello_or_reply_is_refused() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let initiator = start(&chain, 7, NOW);
        for index in [0, 4, 5, 20, HELLO_BYTES - 1] {
            let mut hello = initiator.hello().to_vec();
            hello[index] ^= 1;
            assert!(
                host.accept(&initiator.room(), &hello, [9; 32], [9; 24], NOW)
                    .is_err(),
                "byte {index}"
            );
        }
        assert!(
            host.accept(
                &initiator.room(),
                &initiator.hello()[1..],
                [9; 32],
                [9; 24],
                NOW
            )
            .is_err()
        );
        let reply = accepted(accept(&host, &initiator, 9, NOW)).reply;
        for index in [0, 4, 5, 20, REPLY_BYTES - 1] {
            let mut bad = reply.clone();
            bad[index] ^= 1;
            assert!(
                start(&chain, 7, NOW).finish(&bad, NOW).is_err(),
                "byte {index}"
            );
        }
    }

    #[test]
    fn old_and_future_hellos_are_refused() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let initiator = start(&chain, 7, NOW);
        assert!(accept(&host, &initiator, 9, NOW + 120).is_ok());
        assert!(accept(&host, &initiator, 9, NOW + 121).is_err());
        assert!(accept(&host, &initiator, 9, NOW - 30).is_ok());
        assert!(accept(&host, &initiator, 9, NOW - 31).is_err());
        let reply = accepted(accept(&host, &initiator, 9, NOW)).reply;
        assert!(start(&chain, 7, NOW).finish(&reply, NOW + 121).is_err());
    }

    #[test]
    fn a_reply_belongs_to_one_hello_and_nothing_can_be_reflected() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let first = start(&chain, 7, NOW);
        let second = start(&chain, 8, NOW);
        let reply_for_first = accepted(accept(&host, &first, 9, NOW)).reply;
        assert!(second.finish(&reply_for_first, NOW).is_err());
        // Even with the same session id, a reply is bound to the exact hello bytes.
        let lookalike = Initiator::start(&chain, [8; 32], [7; 16], [8; 24], NOW).unwrap();
        assert!(lookalike.finish(&reply_for_first, NOW).is_err());
        // A hello is not a reply, and a reply is not a hello.
        let other = start(&chain, 7, NOW);
        assert!(other.finish(first.hello(), NOW).is_err());
        assert!(
            host.accept(&first.room(), &reply_for_first, [9; 32], [9; 24], NOW)
                .is_err()
        );
    }

    #[test]
    fn fresh_secrets_give_fresh_keys_and_links() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let run = |seed: u8| {
            let initiator = start(&chain, seed, NOW);
            let accepted = accepted(accept(&host, &initiator, seed + 1, NOW));
            initiator.finish(&accepted.reply, NOW).unwrap().next
        };
        assert_ne!(run(3).as_bytes(), run(4).as_bytes());
    }

    #[test]
    fn a_low_order_peer_key_is_refused() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let mut plain = [7u8; 16].to_vec();
        plain.extend_from_slice(&[0u8; 32]); // the identity point
        plain.extend_from_slice(&NOW.to_be_bytes());
        let hello = seal(
            &chain.hello_key(ROLE_CLIENT),
            ROLE_CLIENT,
            &chain.room_id(),
            &[],
            &[5; 24],
            &plain,
        )
        .unwrap();
        assert_eq!(
            host.accept(&chain.room(), &hello, [9; 32], [9; 24], NOW)
                .err(),
            Some(HelloError::Invalid)
        );
    }

    fn advance(host: &HostChains, client: &Chain, seed: u8) -> (HostChains, Chain) {
        let init = start(client, seed, NOW);
        let mut attempt = accepted(accept(host, &init, seed + 1, NOW));
        let saved = host.after_accept(&attempt).unwrap();
        let mut client = init.finish(attempt.reply(), NOW).unwrap();
        let ready = client.session.seal_batch(&[Frame::Ready]).unwrap();
        (saved.confirm(&mut attempt, &ready).unwrap(), client.next)
    }
    #[test]
    fn repeated_hello_returns_saved_bytes_without_replacing_any_successor() {
        let chain = Chain::from_bytes([1; 32]);
        let init = start(&chain, 3, NOW);
        let attempt = accepted(accept(&HostChains::new(chain.clone()), &init, 4, NOW));
        let host = HostChains::new(chain).after_accept(&attempt).unwrap();
        for now in [NOW, NOW + 1000] {
            let Verdict::CachedReply(reply) = accept(&host, &init, 17, now).unwrap() else {
                panic!("expected cached reply only");
            };
            assert_eq!(reply, attempt.reply());
            assert_eq!(host.pending.len(), 1);
            assert!(host.pending[0].next.as_bytes() == attempt.next.as_bytes());
        }
    }
    #[test]
    fn four_pending_attempts_are_retained_and_a_saved_successor_recovers_at_capacity() {
        let chain = Chain::from_bytes([1; 32]);
        let mut host = HostChains::new(chain.clone());
        for seed in 1..=MAX_PENDING as u8 {
            let init = start(&chain, seed, NOW);
            let attempt = accepted(accept(&host, &init, seed + 9, NOW));
            host = host.after_accept(&attempt).unwrap();
        }
        assert_eq!(host.pending.len(), MAX_PENDING);
        assert_eq!(
            accept(&host, &start(&chain, 20, NOW), 21, NOW).err(),
            Some(HelloError::PendingFull)
        );
        let successor = host.pending[0].next.clone();
        let init = start(&successor, 30, NOW);
        let attempt = accepted(accept(&host, &init, 31, NOW));
        assert_eq!(attempt.via, Via::Pending);
        host = host.after_accept(&attempt).unwrap();
        assert!(host.cur.as_bytes() == successor.as_bytes());
        assert_eq!(host.pending.len(), 1);
    }
    #[test]
    fn confirmation_selects_one_branch_and_cannot_be_rolled_back_by_a_sibling() {
        let chain = Chain::from_bytes([1; 32]);
        let mut host = HostChains::new(chain.clone());
        let first = start(&chain, 1, NOW);
        let second = start(&chain, 3, NOW);
        let mut a = accepted(accept(&host, &first, 2, NOW));
        host = host.after_accept(&a).unwrap();
        let mut b = accepted(accept(&host, &second, 4, NOW));
        host = host.after_accept(&b).unwrap();
        let mut ca = first.finish(a.reply(), NOW).unwrap();
        let mut cb = second.finish(b.reply(), NOW).unwrap();
        let ready = ca.session.seal_batch(&[Frame::Ready]).unwrap();
        host = host.confirm(&mut a, &ready).unwrap();
        assert!(host.pending.is_empty());
        assert!(host.cur.as_bytes() == ca.next.as_bytes());
        assert!(
            host.confirm(&mut b, &cb.session.seal_batch(&[Frame::Ready]).unwrap())
                .is_err()
        );
        assert!(host.after_accept(&b).is_err());
    }
    #[test]
    fn three_retired_generations_warn_but_known_confirmed_hellos_are_ignored() {
        let mut client = Chain::from_bytes([1; 32]);
        let mut host = HostChains::new(client.clone());
        let mut old = Vec::new();
        for seed in [1, 5, 9, 13] {
            old.push((client.clone(), start(&client, seed, NOW)));
            (host, client) = advance(&host, &client, seed);
        }
        assert_eq!(host.retired.len(), MAX_RETIRED);
        assert!(accept(&host, &start(&old[0].0, 40, NOW), 41, NOW).is_err());
        for (chain, known) in old.iter().skip(1) {
            assert!(matches!(
                accept(&host, known, 42, NOW),
                Ok(Verdict::Ignored)
            ));
            let fresh = start(chain, 43, NOW);
            let Verdict::Stale(reply) = accept(&host, &fresh, 44, NOW).unwrap() else {
                panic!("expected stale");
            };
            assert_eq!(fresh.finish(&reply, NOW).err(), Some(HelloError::Stale));
        }
    }
    #[test]
    fn transcript_session_id_matches_and_old_records_cannot_enter_a_new_attempt() {
        let chain = Chain::from_bytes([1; 32]);
        let host = HostChains::new(chain.clone());
        let a = start(&chain, 1, NOW);
        let b = start(&chain, 3, NOW);
        let mut ha = accepted(accept(&host, &a, 2, NOW));
        let hb = accepted(accept(&host, &b, 4, NOW));
        let ca = a.finish(ha.reply(), NOW).unwrap();
        let mut cb = b.finish(hb.reply(), NOW).unwrap();
        assert_eq!(ha.session_id(), ca.session_id());
        assert_eq!(ha.session_id().len(), 43);
        assert_ne!(ha.session_id(), hb.session_id());
        assert!(
            ha.session
                .open_batch(&cb.session.seal_batch(&[Frame::Ready]).unwrap())
                .is_err()
        );
    }
    #[test]
    fn only_an_initial_ready_can_confirm_a_pending_entry() {
        let chain = Chain::from_bytes([1; 32]);
        let host = HostChains::new(chain.clone());
        let init = start(&chain, 1, NOW);
        let mut accepted = accepted(accept(&host, &init, 2, NOW));
        let saved = host.after_accept(&accepted).unwrap();
        let mut client = init.finish(accepted.reply(), NOW).unwrap();
        let record = client
            .session
            .seal_batch(&[Frame::data(1, b"not ready")])
            .unwrap();
        assert!(saved.confirm(&mut accepted, &record).is_err());
        assert!(saved.current().as_bytes() == chain.as_bytes());
    }

    #[test]
    fn failed_ready_is_terminal_and_restored_pending_metadata_is_validated() {
        let chain = Chain::from_bytes([1; 32]);
        let host = HostChains::new(chain.clone());
        let init = start(&chain, 1, NOW);
        let mut attempt = accepted(accept(&host, &init, 2, NOW));
        let saved = host.after_accept(&attempt).unwrap();
        let mut client = init.finish(attempt.reply(), NOW).unwrap();
        let ready = client.session.seal_batch(&[Frame::Ready]).unwrap();
        let mut tampered = ready.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(saved.confirm(&mut attempt, &tampered).is_err());
        assert!(saved.confirm(&mut attempt, &ready).is_err());
        assert!(
            HostChains::restore(chain.clone(), vec![saved.pending[0].clone(); 2], Vec::new())
                .is_err()
        );
        assert!(
            HostChains::restore(
                chain.clone(),
                vec![saved.pending[0].clone(); MAX_PENDING + 1],
                Vec::new()
            )
            .is_err()
        );
        let mut bytes = saved.pending[0].to_bytes();
        bytes[32 + REPLY_BYTES - 1] ^= 1;
        assert!(HostChains::restore(chain, vec![Pending::from_bytes(&bytes)], Vec::new()).is_err());
    }

    #[test]
    fn retired_links_keep_no_secret_that_leads_forward() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let retired = chain.retire();
        assert_eq!(retired.room(), chain.room());
        let restored = Retired::from_bytes(&retired.to_bytes());
        assert_eq!(restored.to_bytes(), retired.to_bytes());
        assert!(
            !retired
                .to_bytes()
                .windows(32)
                .any(|window| window == chain.as_bytes())
        );
    }

    #[test]
    fn batches_round_trip_and_cannot_be_mixed_with_single_records() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let initiator = start(&chain, 1, NOW);
        let mut accepted = accepted(accept(&host, &initiator, 2, NOW));
        let mut client = initiator.finish(&accepted.reply, NOW).unwrap();

        let frames: Vec<Frame> = (1..=40)
            .map(|id| Frame::data(id, format!("token {id}").as_bytes()))
            .collect();
        let record = client.session.seal_batch(&frames).unwrap();
        assert!(!String::from_utf8_lossy(&record).contains("token"));
        let opened = accepted.session.open_batch(&record).unwrap();
        assert_eq!(opened.len(), 40);

        // A single-frame record is not a batch, and a batch is not a single frame.
        let single = client.session.seal(&Frame::Ready).unwrap();
        assert!(accepted.session.open_batch(&single).is_err());
        // The failed open consumed nothing: the same record still opens as the single frame.
        assert!(matches!(
            accepted.session.open(&single).unwrap(),
            Frame::Ready
        ));
        let batch = client.session.seal_batch(&[Frame::Ready]).unwrap();
        assert!(accepted.session.open(&batch).is_err());
        assert!(accepted.session.open_batch(&batch).is_ok());

        // Counters stay strict.
        assert!(accepted.session.open_batch(&record).is_err());
        let _skipped = client.session.seal_batch(&[Frame::Ready]).unwrap();
        let later = client.session.seal_batch(&[Frame::Ready]).unwrap();
        assert!(accepted.session.open_batch(&later).is_err());
    }

    #[test]
    fn batches_have_limits() {
        let chain = Chain::from_code(&code(1)).unwrap();
        let host = HostChains::new(chain.clone());
        let initiator = start(&chain, 1, NOW);
        let accepted = accepted(accept(&host, &initiator, 2, NOW));
        let mut client = initiator.finish(&accepted.reply, NOW).unwrap();
        assert!(client.session.seal_batch(&[]).is_err());
        let many = vec![Frame::Ready; crate::MAX_BATCH_FRAMES + 1];
        assert!(client.session.seal_batch(&many).is_err());
        let big = vec![Frame::data(1, &[7u8; 8 * 1024]); 2];
        assert!(client.session.seal_batch(&big).is_err());
        // Nothing was sealed, so the counter did not move.
        assert_eq!(client.session.send_sequence(), 0);
    }
}
