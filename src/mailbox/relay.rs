//! Bounded volatile ciphertext storage. No payload diagnostics or logging.
use super::{LONG_POLL, PIPELINE, RECORD_TTL, RETRY_HASHES, SESSION_TTL, WINDOW};
use crate::{config::token_matches, diagnostic::Diagnostic};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{
        ConnectInfo, FromRequestParts, Path, Query, Request, State, rejection::QueryRejection,
    },
    http::{Extensions, HeaderMap, StatusCode, request::Parts},
    response::Response,
    routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ipnet::IpNet;
use k_lani_link_protocol::{HELLO_BYTES, MAX_PACKET_BYTES, REPLY_BYTES};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::Notify, time::Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub rooms: usize,
    pub sessions: usize,
    pub bytes: usize,
    pub pending_puts: usize,
    pub pending_put_bytes: usize,
    pub get_waiters: usize,
    pub hello_slots: usize,
    pub requests_per_second: u32,
    pub request_burst: u32,
    /// Open pairing-by-code slots: in `[global]` for all spaces together (default 64), in
    /// a token's limits per space (default 4). 0 turns pairing by code off.
    pub pair_slots: Option<usize>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            rooms: 128,
            sessions: 128,
            bytes: 16 * 1024 * 1024,
            pending_puts: 64,
            pending_put_bytes: 1024 * 1024,
            get_waiters: 64,
            hello_slots: 1024,
            requests_per_second: 1000,
            request_burst: 2000,
            pair_slots: None,
        }
    }
}
impl Limits {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !(1..=100_000).contains(&self.rooms)
            || !(1..=1_000_000).contains(&self.sessions)
            || !(1..=1024 * 1024 * 1024).contains(&self.bytes)
            || !(1..=8192).contains(&self.pending_puts)
            || !(1..=1024 * 1024 * 1024).contains(&self.pending_put_bytes)
            || !(1..=8192).contains(&self.get_waiters)
            || !(1..=1_000_000).contains(&self.hello_slots)
            || !(1..=1_000_000).contains(&self.requests_per_second)
            || !(1..=1_000_000).contains(&self.request_burst)
            || self.pair_slots.is_some_and(|n| n > slots::SLOT_NUMBERS)
        {
            return Err(Diagnostic::err("invalid HTTPS relay limits"));
        }
        Ok(())
    }
}

/// Optional per-token rules, checked after the token matched. Empty: any address, no end.
#[derive(Clone, Default)]
pub struct Access {
    /// Unix seconds from which the token is refused.
    pub expires: Option<u64>,
    /// The peer address must lie in one of these networks.
    pub allow_ips: Vec<IpNet>,
    /// Distinct peer addresses (IPv6 per /64) within 24 hours; 0 means unlimited.
    pub max_ips_per_day: usize,
}
impl Access {
    pub(crate) fn needs_peer(&self) -> bool {
        !self.allow_ips.is_empty() || self.max_ips_per_day > 0
    }
}
/// One admission token. Tokens naming the same space share rooms, quotas and rate; a
/// further token of a space without own limits takes the space's.
pub struct Admit {
    pub key: String,
    pub space: Option<String>,
    pub limits: Option<Limits>,
    pub access: Access,
}
/// Answers `429` to an address after too many refused requests per minute; 0 turns it off.
/// A valid token is never held back by it, so one shared NAT address cannot lock users out.
#[derive(Clone, Copy, Default)]
pub struct Brake {
    pub failures_per_minute: u32,
    pub ban: Duration,
}
/// Bounded bookkeeping: beyond this many addresses, failures are no longer tracked.
const STRIKE_ENTRIES: usize = 4096;
const DAY: Duration = Duration::from_secs(86_400);
struct Strike {
    started: Instant,
    count: u32,
    banned_until: Option<Instant>,
}
/// IPv4 per address, IPv6 per /64: one subscriber usually holds a whole /64.
fn subscriber(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[4..].fill(0);
            IpAddr::V6(segments.into())
        }
        v4 => v4,
    }
}
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}
/// The connection's peer address, when the listener supplies one.
fn peer(extensions: &Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip().to_canonical())
}
struct Peer(Option<IpAddr>);
impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(peer(&parts.extensions)))
    }
}

#[derive(Default)]
struct Counts {
    bytes: AtomicUsize,
    puts: AtomicUsize,
    put_bytes: AtomicUsize,
    gets: AtomicUsize,
}
#[derive(Clone, Copy)]
enum Charge {
    Bytes,
    Put,
    PutBytes,
    Get,
}
impl Counts {
    fn field(&self, kind: Charge) -> &AtomicUsize {
        match kind {
            Charge::Bytes => &self.bytes,
            Charge::Put => &self.puts,
            Charge::PutBytes => &self.put_bytes,
            Charge::Get => &self.gets,
        }
    }
}
struct Lease {
    global: Arc<Counts>,
    tenant: Arc<Counts>,
    kind: Charge,
    amount: usize,
}
impl Lease {
    // Admission is serialized under the relay lock; asynchronous releases only free space.
    fn new(global: &Arc<Counts>, tenant: &Arc<Counts>, kind: Charge, amount: usize) -> Self {
        for counts in [global, tenant] {
            counts.field(kind).fetch_add(amount, Ordering::SeqCst);
        }
        Self {
            global: global.clone(),
            tenant: tenant.clone(),
            kind,
            amount,
        }
    }
    fn shrink(&mut self, amount: usize) {
        assert!(amount <= self.amount);
        for counts in [&self.global, &self.tenant] {
            counts
                .field(self.kind)
                .fetch_sub(self.amount - amount, Ordering::SeqCst);
        }
        self.amount = amount;
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        for counts in [&self.global, &self.tenant] {
            counts
                .field(self.kind)
                .fetch_sub(self.amount, Ordering::SeqCst);
        }
    }
}
struct Packet {
    body: Bytes,
    _bytes: Lease,
}
struct PendingPut {
    _slot: Lease,
    _body: Lease,
    bytes: Option<Lease>,
}
struct Entry {
    created: Instant,
    packet: Arc<Packet>,
}
#[derive(Default)]
struct Channel {
    floor: u64,
    offered: u64,
    records: BTreeMap<u64, Entry>,
    acknowledged: VecDeque<(u64, [u8; 32])>,
    uploads: Arc<AtomicUsize>,
}
impl Channel {
    fn new() -> Self {
        Self {
            floor: 1,
            ..Self::default()
        }
    }
    fn expire(&mut self) -> bool {
        let before = self.records.len();
        self.records.retain(|index, entry| {
            if entry.created.elapsed() < Duration::from_secs(RECORD_TTL) {
                true
            } else {
                self.floor = self.floor.max(index.saturating_add(1));
                false
            }
        });
        self.records.len() != before
    }
}
struct Session {
    updated: Instant,
    channels: [Channel; 2],
}
struct Room {
    updated: Instant,
    client: Option<Entry>,
    replies: HashMap<String, Entry>,
    sessions: HashMap<String, Session>,
}
impl Room {
    fn new() -> Self {
        Self {
            updated: Instant::now(),
            client: None,
            replies: HashMap::new(),
            sessions: HashMap::new(),
        }
    }
}
struct Bucket {
    tokens: f64,
    updated: Instant,
}
impl Bucket {
    fn new(limits: &Limits) -> Self {
        Self {
            tokens: limits.request_burst as f64,
            updated: Instant::now(),
        }
    }
    fn refill(&mut self, limits: &Limits) {
        self.tokens = (self.tokens
            + self.updated.elapsed().as_secs_f64() * limits.requests_per_second as f64)
            .min(limits.request_burst as f64);
        self.updated = Instant::now();
    }
}
// Waiters register per room (handshake) or per direction (records); a change wakes only its key.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Wake {
    Room(usize, String),
    Channel(usize, String, String, usize),
    Pair(u16),
}
struct Inner {
    rooms: HashMap<(usize, String), Room>,
    global_rate: Bucket,
    rates: Vec<Bucket>,
    wakes: HashMap<Wake, std::sync::Weak<Notify>>,
    /// Per token: salted hashes of the addresses seen within 24 hours, never the addresses.
    seen: Vec<HashMap<[u8; 16], Instant>>,
    strikes: HashMap<IpAddr, Strike>,
    /// Pairing-by-code slots by their three-digit number.
    slots: HashMap<u16, slots::Slot>,
    /// The tokenless side of the slots has its own budget, never the tunnels'.
    door_rate: Bucket,
}
/// A namespace of rooms with its own quotas; one or more tokens lead into it.
struct Tenant {
    name: Option<String>,
    limits: Limits,
    counts: Arc<Counts>,
}
struct Token {
    key: String,
    tenant: usize,
    access: Access,
}
pub struct Relay {
    limits: Limits,
    tenants: Vec<Tenant>,
    tokens: Vec<Token>,
    brake: Brake,
    salt: [u8; 32],
    counts: Arc<Counts>,
    inner: Mutex<Inner>,
}
type Failure = StatusCode;
type Answer<T> = Result<T, Failure>;

fn id(value: &str, bytes: usize) -> bool {
    URL_SAFE_NO_PAD
        .decode(value)
        .is_ok_and(|decoded| decoded.len() == bytes && URL_SAFE_NO_PAD.encode(decoded) == value)
}
pub fn hello_hash(hello: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(hello))
}
pub fn session_id(hello: &[u8], reply: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"k-lani-link/mailbox/transcript/v1");
    for value in [hello, reply] {
        hash.update((value.len() as u32).to_be_bytes());
        hash.update(value);
    }
    URL_SAFE_NO_PAD.encode(hash.finalize())
}
fn direction(value: &str) -> Answer<usize> {
    match value {
        "c2h" => Ok(0),
        "h2c" => Ok(1),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}
fn fits(value: usize, extra: usize, limit: usize) -> bool {
    value.checked_add(extra).is_some_and(|n| n <= limit)
}
impl Relay {
    /// One space per token, no access rules.
    pub fn new(limits: Limits, tokens: Vec<(String, Limits)>) -> anyhow::Result<Arc<Self>> {
        let tokens = tokens
            .into_iter()
            .map(|(key, limits)| Admit {
                key,
                space: None,
                limits: Some(limits),
                access: Access::default(),
            })
            .collect();
        Self::with_access(limits, tokens, Brake::default())
    }
    pub fn with_access(
        limits: Limits,
        admits: Vec<Admit>,
        brake: Brake,
    ) -> anyhow::Result<Arc<Self>> {
        limits.validate()?;
        if admits.is_empty() || admits.len() > 64 {
            return Err(Diagnostic::err(
                "HTTPS relay requires 1..64 admission tokens",
            ));
        }
        let mut tenants: Vec<Tenant> = Vec::new();
        let mut tokens: Vec<Token> = Vec::new();
        for admit in admits {
            if let Some(limits) = &admit.limits {
                limits.validate()?;
            }
            let key = admit.key;
            if key.len() < 32
                || key.len() > 256
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || tokens.iter().any(|previous| previous.key == key)
            {
                return Err(Diagnostic::err(
                    "invalid or duplicate HTTPS relay admission token",
                ));
            }
            let shared = admit.space.as_ref().and_then(|name| {
                tenants
                    .iter()
                    .position(|tenant| tenant.name.as_ref() == Some(name))
            });
            let tenant = match shared {
                Some(index)
                    if admit
                        .limits
                        .as_ref()
                        .is_some_and(|l| *l != tenants[index].limits) =>
                {
                    return Err(Diagnostic::err(
                        "admission tokens of one space need identical limits",
                    ));
                }
                Some(index) => index,
                None => {
                    tenants.push(Tenant {
                        name: admit.space,
                        limits: admit.limits.unwrap_or_default(),
                        counts: Arc::default(),
                    });
                    tenants.len() - 1
                }
            };
            tokens.push(Token {
                key,
                tenant,
                access: admit.access,
            });
        }
        let mut salt = [0u8; 32];
        getrandom::getrandom(&mut salt).map_err(|_| Diagnostic::err("OS randomness failed"))?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Inner {
                rooms: HashMap::new(),
                global_rate: Bucket::new(&limits),
                rates: tenants.iter().map(|t| Bucket::new(&t.limits)).collect(),
                wakes: HashMap::new(),
                seen: tokens.iter().map(|_| HashMap::new()).collect(),
                strikes: HashMap::new(),
                slots: HashMap::new(),
                door_rate: Bucket::new(&slots::door_limits()),
            }),
            limits,
            tenants,
            tokens,
            brake,
            salt,
            counts: Arc::default(),
        }))
    }
    pub fn router(self: &Arc<Self>, stop: CancellationToken) -> Router {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = tick.tick() => {} }
                let Some(relay) = weak.upgrade() else { break };
                relay.sweep();
            }
        });
        Router::new()
            // POST is accepted as a write too, for clients behind PUT-blocking firewalls.
            .route(
                "/v1/hello/{room}/client",
                get(get_client).put(put_client).post(put_client),
            )
            .route(
                "/v1/hello/{room}/host/{hash}",
                get(get_reply).put(put_reply).post(put_reply),
            )
            .route(
                "/v1/box/{room}/{session}/{direction}/{sequence}",
                axum::routing::put(put_record).post(put_record),
            )
            .route("/v1/box/{room}/{session}/{direction}", get(get_records))
            .route("/v1/pair", axum::routing::post(slots::open_pair))
            .route("/v1/pair/{slot}", axum::routing::delete(slots::close_pair))
            .route(
                "/v1/pair/{slot}/{part}",
                get(slots::read_pair)
                    .put(slots::write_pair)
                    .post(slots::write_pair),
            )
            .with_state(self.clone())
    }
    /// Expires one room in place and names the waiters that must re-check. False: remove it.
    fn sweep_room(tenant: usize, name: &str, room: &mut Room, changed: &mut Vec<Wake>) -> bool {
        let ttl = Duration::from_secs(SESSION_TTL);
        if room.updated.elapsed() >= ttl {
            changed.push(Wake::Room(tenant, name.into()));
            for session in room.sessions.keys() {
                for dir in 0..2 {
                    changed.push(Wake::Channel(tenant, name.into(), session.clone(), dir));
                }
            }
            return false;
        }
        if room
            .client
            .as_ref()
            .is_some_and(|e| e.created.elapsed() >= Duration::from_secs(RECORD_TTL))
        {
            room.client = None;
        }
        room.replies
            .retain(|_, e| e.created.elapsed() < Duration::from_secs(RECORD_TTL));
        room.sessions.retain(|session, state| {
            let removed = state.updated.elapsed() >= ttl;
            for (dir, channel) in state.channels.iter_mut().enumerate() {
                if removed || channel.expire() {
                    changed.push(Wake::Channel(tenant, name.into(), session.clone(), dir));
                }
            }
            !removed
        });
        true
    }
    fn sweep_inner(inner: &mut Inner, changed: &mut Vec<Wake>) {
        inner
            .rooms
            .retain(|(tenant, name), room| Self::sweep_room(*tenant, name, room, changed));
    }
    /// Expires only the addressed room first, so a late request cannot refresh expired state.
    fn touch<'a>(
        inner: &'a mut Inner,
        tenant: usize,
        name: &str,
        changed: &mut Vec<Wake>,
    ) -> Option<&'a mut Room> {
        let key = (tenant, name.to_owned());
        if !Self::sweep_room(tenant, name, inner.rooms.get_mut(&key)?, changed) {
            inner.rooms.remove(&key);
            return None;
        }
        inner.rooms.get_mut(&key)
    }
    /// Runs one state transition under the lock, then wakes only the waiters it names.
    fn with_state<T>(
        &self,
        work: impl FnOnce(&mut Inner, &mut Vec<Wake>) -> Answer<T>,
    ) -> Answer<T> {
        let mut changed = Vec::new();
        let mut inner = self.inner.lock().unwrap();
        let result = work(&mut inner, &mut changed);
        let wakes: Vec<_> = changed
            .iter()
            .filter_map(|key| inner.wakes.get(key).and_then(std::sync::Weak::upgrade))
            .collect();
        drop(inner);
        for wake in wakes {
            wake.notify_waiters();
        }
        result
    }
    /// Admitted requests only: each holds one registration while it waits.
    fn waiter(&self, key: Wake) -> Arc<Notify> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(wake) = inner.wakes.get(&key).and_then(std::sync::Weak::upgrade) {
            return wake;
        }
        let wake = Arc::new(Notify::new());
        inner.wakes.insert(key, Arc::downgrade(&wake));
        wake
    }
    pub fn sweep(&self) {
        let _ = self.with_state(|inner, changed| {
            Self::sweep_inner(inner, changed);
            inner.slots.retain(|number, slot| {
                let keep = !slot.expired();
                if !keep {
                    changed.push(Wake::Pair(*number));
                }
                keep
            });
            inner.wakes.retain(|_, wake| wake.strong_count() > 0);
            for seen in &mut inner.seen {
                seen.retain(|_, first| first.elapsed() < DAY);
            }
            inner.strikes.retain(|_, strike| {
                strike.started.elapsed() < Duration::from_secs(60)
                    || strike
                        .banned_until
                        .is_some_and(|until| until > Instant::now())
            });
            Ok(())
        });
    }
    /// The token's space, after its access rules and the request rate allow the request.
    fn authorize(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> Answer<usize> {
        let header = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok());
        let mut selected = None;
        for (index, token) in self.tokens.iter().enumerate() {
            if token_matches(header, &token.key) {
                selected = Some(index);
            }
        }
        let mut inner = self.inner.lock().unwrap();
        let Some(index) = selected.filter(|&index| self.admits(&mut inner, index, peer)) else {
            return Err(self.strike(&mut inner, peer));
        };
        let tenant = self.tokens[index].tenant;
        inner.global_rate.refill(&self.limits);
        inner.rates[tenant].refill(&self.tenants[tenant].limits);
        if inner.global_rate.tokens < 1.0 || inner.rates[tenant].tokens < 1.0 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        inner.global_rate.tokens -= 1.0;
        inner.rates[tenant].tokens -= 1.0;
        Ok(tenant)
    }
    /// Expiry, address networks and addresses per day. Without a known peer address,
    /// a token with address rules is refused.
    fn admits(&self, inner: &mut Inner, index: usize, peer: Option<IpAddr>) -> bool {
        let access = &self.tokens[index].access;
        if access.expires.is_some_and(|end| unix_now() >= end) {
            return false;
        }
        if !access.needs_peer() {
            return true;
        }
        let Some(ip) = peer.map(|ip| ip.to_canonical()) else {
            return false;
        };
        if !access.allow_ips.is_empty() && !access.allow_ips.iter().any(|net| net.contains(&ip)) {
            return false;
        }
        if access.max_ips_per_day > 0 {
            let seen = &mut inner.seen[index];
            seen.retain(|_, first| first.elapsed() < DAY);
            let mut hash = Sha256::new();
            hash.update(self.salt);
            match subscriber(ip) {
                IpAddr::V4(v4) => hash.update(v4.octets()),
                IpAddr::V6(v6) => hash.update(v6.octets()),
            }
            let key: [u8; 16] = hash.finalize()[..16].try_into().unwrap();
            if !seen.contains_key(&key) {
                if seen.len() >= access.max_ips_per_day {
                    return false;
                }
                seen.insert(key, Instant::now());
            }
        }
        true
    }
    /// Counts a refused request against its address: `429` while banned, else `401`.
    fn strike(&self, inner: &mut Inner, peer: Option<IpAddr>) -> Failure {
        let (Some(ip), 1..) = (peer, self.brake.failures_per_minute) else {
            return StatusCode::UNAUTHORIZED;
        };
        let key = subscriber(ip);
        if !inner.strikes.contains_key(&key) && inner.strikes.len() >= STRIKE_ENTRIES {
            return StatusCode::UNAUTHORIZED;
        }
        let now = Instant::now();
        let strike = inner.strikes.entry(key).or_insert(Strike {
            started: now,
            count: 0,
            banned_until: None,
        });
        if strike.banned_until.is_some_and(|until| now < until) {
            return StatusCode::TOO_MANY_REQUESTS;
        }
        if now.duration_since(strike.started) >= Duration::from_secs(60) {
            *strike = Strike {
                started: now,
                count: 0,
                banned_until: None,
            };
        }
        strike.count += 1;
        if strike.count > self.brake.failures_per_minute {
            strike.banned_until = Some(now + self.brake.ban);
            return StatusCode::TOO_MANY_REQUESTS;
        }
        StatusCode::UNAUTHORIZED
    }
    fn charge(&self, tenant: usize, kind: Charge, amount: usize) -> Answer<Lease> {
        let entry = &self.tenants[tenant];
        let bound = |limits: &Limits| match kind {
            Charge::Bytes => limits.bytes,
            Charge::Put => limits.pending_puts,
            Charge::PutBytes => limits.pending_put_bytes,
            Charge::Get => limits.get_waiters,
        };
        if !fits(
            self.counts.field(kind).load(Ordering::SeqCst),
            amount,
            bound(&self.limits),
        ) || !fits(
            entry.counts.field(kind).load(Ordering::SeqCst),
            amount,
            bound(&entry.limits),
        ) {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        Ok(Lease::new(&self.counts, &entry.counts, kind, amount))
    }
    fn upload(&self, tenant: usize, maximum: usize) -> Answer<PendingPut> {
        let _inner = self.inner.lock().unwrap();
        let slot = self.charge(tenant, Charge::Put, 1)?;
        let body = self.charge(tenant, Charge::PutBytes, maximum)?;
        let bytes = self.charge(tenant, Charge::Bytes, maximum)?;
        Ok(PendingPut {
            _slot: slot,
            _body: body,
            bytes: Some(bytes),
        })
    }
    fn getter(&self, tenant: usize) -> Answer<Lease> {
        let _inner = self.inner.lock().unwrap();
        self.charge(tenant, Charge::Get, 1)
    }
    fn room_capacity(&self, inner: &Inner, tenant: usize, room: &str) -> Answer<()> {
        if inner.rooms.contains_key(&(tenant, room.into())) {
            return Ok(());
        }
        if inner.rooms.len() >= self.limits.rooms
            || inner.rooms.keys().filter(|(t, _)| *t == tenant).count()
                >= self.tenants[tenant].limits.rooms
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        Ok(())
    }
    fn slot_capacity(&self, inner: &Inner, tenant: usize) -> Answer<()> {
        let count = |selected: Option<usize>| {
            inner
                .rooms
                .iter()
                .filter(|((t, _), _)| selected.is_none_or(|s| *t == s))
                .map(|(_, r)| r.replies.len() + usize::from(r.client.is_some()))
                .sum::<usize>()
        };
        if count(None) >= self.limits.hello_slots
            || count(Some(tenant)) >= self.tenants[tenant].limits.hello_slots
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        Ok(())
    }
    fn session_capacity(&self, inner: &Inner, tenant: usize) -> Answer<()> {
        let count = |selected: Option<usize>| {
            inner
                .rooms
                .iter()
                .filter(|((t, _), _)| selected.is_none_or(|s| *t == s))
                .map(|(_, r)| r.sessions.len())
                .sum::<usize>()
        };
        if count(None) >= self.limits.sessions
            || count(Some(tenant)) >= self.tenants[tenant].limits.sessions
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        Ok(())
    }
    fn write_hello(
        &self,
        tenant: usize,
        room: &str,
        hash: Option<&str>,
        body: Bytes,
        mut reservation: PendingPut,
    ) -> Answer<()> {
        let expected = if hash.is_some() {
            REPLY_BYTES
        } else {
            HELLO_BYTES
        };
        let role = if hash.is_some() { 2 } else { 1 };
        if !id(room, 16)
            || hash.is_some_and(|h| !id(h, 32))
            || body.len() != expected
            || &body[..4] != b"KLH1"
            || body[4] != role
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        self.with_state(|inner, changed| {
            // Rare handshake path: a full sweep keeps the capacity counts exact.
            Self::sweep_inner(inner, changed);
            self.room_capacity(inner, tenant, room)?;
            let key = (tenant, room.into());
            let old = inner.rooms.get(&key).and_then(|r| match hash {
                Some(h) => r.replies.get(h),
                None => r.client.as_ref(),
            });
            if let Some(old) = old {
                if old.packet.body == body {
                    return Ok(());
                }
                if hash.is_some() {
                    return Err(StatusCode::CONFLICT);
                }
            } else {
                self.slot_capacity(inner, tenant)?;
            }
            let session = if let Some(hash) = hash {
                let hello = inner
                    .rooms
                    .get(&key)
                    .and_then(|r| r.client.as_ref())
                    .ok_or(StatusCode::GONE)?;
                if hello_hash(&hello.packet.body) != hash {
                    return Err(StatusCode::GONE);
                }
                let session = session_id(&hello.packet.body, &body);
                if !inner.rooms[&key].sessions.contains_key(&session) {
                    self.session_capacity(inner, tenant)?;
                }
                Some(session)
            } else {
                None
            };
            reservation.bytes.as_mut().unwrap().shrink(body.len());
            let packet = Arc::new(Packet {
                body,
                _bytes: reservation.bytes.take().unwrap(),
            });
            let state = inner.rooms.entry(key).or_insert_with(Room::new);
            state.updated = Instant::now();
            let entry = Entry {
                created: Instant::now(),
                packet,
            };
            if let Some(hash) = hash {
                state.replies.insert(hash.into(), entry);
                state
                    .sessions
                    .entry(session.unwrap())
                    .or_insert_with(|| Session {
                        updated: Instant::now(),
                        channels: [Channel::new(), Channel::new()],
                    });
            } else {
                state.client = Some(entry);
            }
            changed.push(Wake::Room(tenant, room.into()));
            Ok(())
        })
    }
    fn hello(&self, tenant: usize, room: &str, hash: Option<&str>) -> Option<Arc<Packet>> {
        self.with_state(|inner, changed| {
            let Some(state) = Self::touch(inner, tenant, room, changed) else {
                return Ok(None);
            };
            state.updated = Instant::now();
            Ok(match hash {
                Some(h) => state.replies.get(h),
                None => state.client.as_ref(),
            }
            .map(|e| e.packet.clone()))
        })
        .ok()
        .flatten()
    }
    fn store_record(
        &self,
        tenant: usize,
        room: &str,
        session: &str,
        dir: usize,
        sequence: u64,
        packet: &Arc<Packet>,
    ) -> Answer<bool> {
        self.with_state(|inner, changed| {
            let state = Self::touch(inner, tenant, room, changed).ok_or(StatusCode::GONE)?;
            let entry = state.sessions.get_mut(session).ok_or(StatusCode::GONE)?;
            let channel = &mut entry.channels[dir];
            if sequence < channel.floor {
                return match channel
                    .acknowledged
                    .iter()
                    .find(|(index, _)| *index == sequence)
                {
                    Some((_, hash)) if *hash == <[u8; 32]>::from(Sha256::digest(&packet.body)) => {
                        Ok(true)
                    }
                    Some(_) => Err(StatusCode::CONFLICT),
                    None => Err(StatusCode::GONE),
                };
            }
            if let Some(old) = channel.records.get(&sequence) {
                return if old.packet.body == packet.body {
                    Ok(true)
                } else {
                    Err(StatusCode::CONFLICT)
                };
            }
            // Keep a slot for a missing earlier PUT; a fast later PUT cannot deadlock its predecessor.
            if sequence - channel.floor >= WINDOW as u64 || channel.records.len() >= WINDOW {
                return Ok(false);
            }
            channel.records.insert(
                sequence,
                Entry {
                    created: Instant::now(),
                    packet: packet.clone(),
                },
            );
            entry.updated = Instant::now();
            state.updated = Instant::now();
            changed.push(Wake::Channel(tenant, room.into(), session.into(), dir));
            Ok(true)
        })
    }
    fn records(
        &self,
        tenant: usize,
        room: &str,
        session: &str,
        dir: usize,
        from: u64,
    ) -> Answer<Vec<Arc<Packet>>> {
        self.with_state(|inner, changed| {
            let state = Self::touch(inner, tenant, room, changed).ok_or(StatusCode::GONE)?;
            let entry = state.sessions.get_mut(session).ok_or(StatusCode::GONE)?;
            let channel = &mut entry.channels[dir];
            if from < channel.floor {
                return Err(StatusCode::GONE);
            }
            if from == 0 || from > channel.offered.saturating_add(1) {
                return Err(StatusCode::BAD_REQUEST);
            }
            let indices: Vec<_> = channel
                .records
                .range(..from)
                .map(|(index, _)| *index)
                .collect();
            let had_records = !indices.is_empty();
            for index in indices {
                let record = channel.records.remove(&index).unwrap();
                channel
                    .acknowledged
                    .push_back((index, Sha256::digest(&record.packet.body).into()));
                if channel.acknowledged.len() > RETRY_HASHES {
                    channel.acknowledged.pop_front();
                }
            }
            let freed = had_records || from > channel.floor;
            channel.floor = from;
            let mut packets = Vec::new();
            let mut index = from;
            for _ in 0..WINDOW {
                let Some(record) = channel.records.get(&index) else {
                    break;
                };
                packets.push(record.packet.clone());
                channel.offered = channel.offered.max(index);
                let Some(next) = index.checked_add(1) else {
                    break;
                };
                index = next;
            }
            entry.updated = Instant::now();
            state.updated = Instant::now();
            if freed {
                changed.push(Wake::Channel(tenant, room.into(), session.into(), dir));
            }
            Ok(packets)
        })
    }
    fn response(&self, status: StatusCode, body: Body) -> Response {
        let mut response = Response::new(body);
        *response.status_mut() = status;
        let headers = response.headers_mut();
        headers.insert("cache-control", "no-store, no-transform".parse().unwrap());
        headers.insert("content-type", "application/octet-stream".parse().unwrap());
        headers.insert("x-content-type-options", "nosniff".parse().unwrap());
        if status == StatusCode::TOO_MANY_REQUESTS {
            headers.insert("retry-after", "1".parse().unwrap());
        }
        response
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wait {
    #[serde(default = "default_wait")]
    wait: u64,
}
fn default_wait() -> u64 {
    LONG_POLL
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    from: u64,
    #[serde(default = "default_wait")]
    wait: u64,
}
/// A write's body: raw bytes, or `{"data":"<base64url>"}` from clients behind web
/// firewalls that only admit JSON. Both carry the identical sealed record.
async fn body(request: Request, limit: usize, deadline: Instant) -> Answer<Bytes> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wrapped {
        data: String,
    }
    let json = wrapped(request.headers());
    let wire = if json { wire_limit(limit) } else { limit };
    let bytes = tokio::time::timeout_at(deadline, to_bytes(request.into_body(), wire))
        .await
        .map_err(|_| StatusCode::REQUEST_TIMEOUT)?
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    if !json {
        return Ok(bytes);
    }
    let wrapped: Wrapped = serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    let decoded = URL_SAFE_NO_PAD
        .decode(wrapped.data)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if decoded.len() > limit {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    Ok(Bytes::from(decoded))
}
fn wrapped(headers: &HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next().unwrap_or("").trim() == "application/json")
}
/// Bytes of a JSON-wrapped body of `limit` raw bytes.
fn wire_limit(limit: usize) -> usize {
    limit.div_ceil(3) * 4 + 16
}
async fn write_hello(
    relay: &Relay,
    room: &str,
    hash: Option<&str>,
    request: Request,
) -> Answer<()> {
    let tenant = relay.authorize(request.headers(), peer(request.extensions()))?;
    if !id(room, 16) || hash.is_some_and(|h| !id(h, 32)) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let size = if hash.is_some() {
        REPLY_BYTES
    } else {
        HELLO_BYTES
    };
    let wire = if wrapped(request.headers()) {
        wire_limit(size)
    } else {
        size
    };
    let reservation = relay.upload(tenant, wire)?;
    let bytes = body(
        request,
        size,
        Instant::now() + Duration::from_secs(RECORD_TTL),
    )
    .await?;
    relay.write_hello(tenant, room, hash, bytes, reservation)
}
async fn put_client(
    State(relay): State<Arc<Relay>>,
    Path(room): Path<String>,
    request: Request,
) -> Response {
    let status = write_hello(&relay, &room, None, request)
        .await
        .err()
        .unwrap_or(StatusCode::NO_CONTENT);
    relay.response(status, Body::empty())
}
async fn put_reply(
    State(relay): State<Arc<Relay>>,
    Path((room, hash)): Path<(String, String)>,
    request: Request,
) -> Response {
    let status = write_hello(&relay, &room, Some(&hash), request)
        .await
        .err()
        .unwrap_or(StatusCode::NO_CONTENT);
    relay.response(status, Body::empty())
}
async fn read_hello(
    relay: &Relay,
    room: &str,
    hash: Option<&str>,
    (headers, address): (HeaderMap, Option<IpAddr>),
    query: Result<Query<Wait>, QueryRejection>,
) -> Response {
    let work = async {
        let tenant = relay.authorize(&headers, address)?;
        let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
        if !id(room, 16) || hash.is_some_and(|h| !id(h, 32)) || query.wait > LONG_POLL {
            return Err(StatusCode::BAD_REQUEST);
        }
        let lease = relay.getter(tenant)?;
        let deadline = Instant::now() + Duration::from_secs(query.wait);
        let wake = relay.waiter(Wake::Room(tenant, room.into()));
        loop {
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(packet) = relay.hello(tenant, room, hash) {
                return Ok(Some((packet, lease)));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }
    .await;
    match work {
        Ok(Some((packet, lease))) => {
            let stream = async_stream::stream! {
                let _lease = lease;
                yield Ok::<_, std::convert::Infallible>(Bytes::from_owner(PacketBody(packet)));
            };
            relay.response(StatusCode::OK, Body::from_stream(stream))
        }
        Ok(None) => relay.response(StatusCode::NO_CONTENT, Body::empty()),
        Err(status) => relay.response(status, Body::empty()),
    }
}
async fn get_client(
    State(relay): State<Arc<Relay>>,
    Path(room): Path<String>,
    Peer(address): Peer,
    headers: HeaderMap,
    query: Result<Query<Wait>, QueryRejection>,
) -> Response {
    read_hello(&relay, &room, None, (headers, address), query).await
}
async fn get_reply(
    State(relay): State<Arc<Relay>>,
    Path((room, hash)): Path<(String, String)>,
    Peer(address): Peer,
    headers: HeaderMap,
    query: Result<Query<Wait>, QueryRejection>,
) -> Response {
    read_hello(&relay, &room, Some(&hash), (headers, address), query).await
}
async fn put_record(
    State(relay): State<Arc<Relay>>,
    Path((room, session, dir, sequence)): Path<(String, String, String, u64)>,
    request: Request,
) -> Response {
    let work = async {
        let tenant = relay.authorize(request.headers(), peer(request.extensions()))?;
        let dir = direction(&dir)?;
        if !id(&room, 16) || !id(&session, 32) || sequence == 0 {
            return Err(StatusCode::BAD_REQUEST);
        }
        let deadline = Instant::now() + Duration::from_secs(RECORD_TTL);
        let _direction =
            DirectionLease::new(relay.clone(), (tenant, room.clone(), session.clone(), dir))?;
        let wire = if wrapped(request.headers()) {
            wire_limit(MAX_PACKET_BYTES)
        } else {
            MAX_PACKET_BYTES
        };
        let mut reservation = relay.upload(tenant, wire)?;
        let bytes = body(request, MAX_PACKET_BYTES, deadline).await?;
        if bytes.len() < 28
            || &bytes[..4] != b"KLI2"
            || u64::from_be_bytes(bytes[4..12].try_into().unwrap()) != sequence - 1
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        reservation.bytes.as_mut().unwrap().shrink(bytes.len());
        let packet = Arc::new(Packet {
            body: bytes,
            _bytes: reservation.bytes.take().unwrap(),
        });
        let wake = relay.waiter(Wake::Channel(tenant, room.clone(), session.clone(), dir));
        loop {
            if Instant::now() >= deadline {
                return Err(StatusCode::REQUEST_TIMEOUT);
            }
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if relay.store_record(tenant, &room, &session, dir, sequence, &packet)? {
                return Ok(());
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(StatusCode::REQUEST_TIMEOUT);
            }
        }
    }
    .await;
    relay.response(work.err().unwrap_or(StatusCode::NO_CONTENT), Body::empty())
}

mod slots;

#[cfg(test)]
mod tests;
type DirectionKey = (usize, String, String, usize);
// Separate from ciphertext storage so cancelled uploads cannot leave queue entries behind.
struct DirectionLease {
    count: Arc<AtomicUsize>,
    relay: std::sync::Weak<Relay>,
    key: DirectionKey,
}
impl DirectionLease {
    fn new(relay: Arc<Relay>, key: DirectionKey) -> Answer<Self> {
        let inner = relay.inner.lock().unwrap();
        let count = inner
            .rooms
            .get(&(key.0, key.1.clone()))
            .and_then(|r| r.sessions.get(&key.2))
            .ok_or(StatusCode::GONE)?
            .channels[key.3]
            .uploads
            .clone();
        if count.load(Ordering::SeqCst) >= PIPELINE {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        count.fetch_add(1, Ordering::SeqCst);
        drop(inner);
        Ok(Self {
            count,
            relay: Arc::downgrade(&relay),
            key,
        })
    }
}
impl Drop for DirectionLease {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        if let Some(relay) = self.relay.upgrade() {
            let (tenant, room, session, dir) = self.key.clone();
            let _ = relay.with_state(|_, changed| {
                changed.push(Wake::Channel(tenant, room, session, dir));
                Ok(())
            });
        }
    }
}
// Bytes held by Hyper remain charged even after a response or retained entry is dropped.
struct PacketBody(Arc<Packet>);
impl AsRef<[u8]> for PacketBody {
    fn as_ref(&self) -> &[u8] {
        &self.0.body
    }
}
async fn get_records(
    State(relay): State<Arc<Relay>>,
    Path((room, session, dir)): Path<(String, String, String)>,
    Peer(address): Peer,
    headers: HeaderMap,
    query: Result<Query<Cursor>, QueryRejection>,
) -> Response {
    let work = async {
        let tenant = relay.authorize(&headers, address)?;
        let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
        let dir = direction(&dir)?;
        if !id(&room, 16) || !id(&session, 32) || query.wait > LONG_POLL || query.from == 0 {
            return Err(StatusCode::BAD_REQUEST);
        }
        let lease = relay.getter(tenant)?;
        let deadline = Instant::now() + Duration::from_secs(query.wait);
        let wake = relay.waiter(Wake::Channel(tenant, room.clone(), session.clone(), dir));
        loop {
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let packets = relay.records(tenant, &room, &session, dir, query.from)?;
            if !packets.is_empty() {
                return Ok(Some((packets, lease)));
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }
    .await;
    match work {
        Ok(Some((packets, lease))) => {
            let length: usize = packets.iter().map(|p| 4 + p.body.len()).sum();
            let stream = async_stream::stream! {
                let _lease = lease;
                for packet in packets {
                    yield Ok::<_, std::convert::Infallible>(Bytes::copy_from_slice(&(packet.body.len() as u32).to_be_bytes()));
                    yield Ok(Bytes::from_owner(PacketBody(packet)));
                }
            };
            let mut response = relay.response(StatusCode::OK, Body::from_stream(stream));
            response
                .headers_mut()
                .insert("content-length", length.to_string().parse().unwrap());
            response
        }
        Ok(None) => relay.response(StatusCode::NO_CONTENT, Body::empty()),
        Err(status) => relay.response(status, Body::empty()),
    }
}
