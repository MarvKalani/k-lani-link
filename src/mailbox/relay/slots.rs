//! Pairing by code: short-lived slots that carry one password-authenticated key exchange.
//!
//! A host with an admission token opens a slot and gets a three-digit number. The client
//! knows only the code, so the slot's client side is the one narrow route without a token:
//! it can read the host's opening message, write ONE answer, read the host's sealed bundle
//! and write one sealed acknowledgement. Unknown, expired and used slots all answer 404 and
//! count against the failure brake. Everything here is ciphertext or a public PAKE message.
use super::*;

/// A slot lives this long; the code shown to the user is useless afterwards.
pub(crate) const PAIR_TTL: Duration = Duration::from_secs(600);
/// Slot numbers run from 100 to 999, so no relay can hold more open slots than this.
pub(super) const SLOT_NUMBERS: usize = 900;
/// Without `pair_slots`: open slots on the whole relay, and per space.
const PAIR_SLOTS: usize = 64;
const PAIR_PER_SPACE: usize = 4;
/// Opening message, client answer, sealed bundle, sealed acknowledgement.
const PART_BYTES: [usize; 4] = [64, 128, 8192, 64];
/// Requests per second for everyone without a token together. A pairing client polls
/// once a second, so this serves a handful of pairings at a time.
pub(super) fn door_limits() -> Limits {
    Limits {
        requests_per_second: 20,
        request_burst: 40,
        ..Limits::default()
    }
}

pub(super) struct Slot {
    tenant: usize,
    created: Instant,
    parts: [Option<Bytes>; 4],
}
impl Slot {
    pub(super) fn expired(&self) -> bool {
        self.created.elapsed() >= PAIR_TTL
    }
}

fn part(name: &str) -> Option<usize> {
    ["a", "b", "c", "d"].iter().position(|p| *p == name)
}
fn slot_number(text: &str) -> Option<u16> {
    (text.len() == 3 && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
        .filter(|n| (100..=999).contains(n))
}
/// Parts the client writes or reads without a token: a, b (write), c, d (write).
fn tokenless(index: usize, write: bool) -> bool {
    matches!(
        (index, write),
        (0, false) | (1, true) | (2, false) | (3, true)
    )
}

impl Relay {
    /// A banned address gets 429 before anything else; others pay one token of the
    /// tokenless budget, which the tunnels' global and per-space budgets never share.
    pub(super) fn open_door(&self, peer: Option<IpAddr>) -> Answer<()> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(ip) = peer
            && inner
                .strikes
                .get(&subscriber(ip))
                .and_then(|strike| strike.banned_until)
                .is_some_and(|until| Instant::now() < until)
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        inner.door_rate.refill(&door_limits());
        if inner.door_rate.tokens < 1.0 {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        inner.door_rate.tokens -= 1.0;
        Ok(())
    }
    /// The one answer for an unknown, expired or used slot: 404, or 429 once banned.
    fn no_slot(&self, peer: Option<IpAddr>) -> Failure {
        let mut inner = self.inner.lock().unwrap();
        match self.strike(&mut inner, peer) {
            StatusCode::TOO_MANY_REQUESTS => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::NOT_FOUND,
        }
    }
    pub(super) fn open_slot(&self, tenant: usize, opening: Bytes) -> Answer<u16> {
        if opening.is_empty() || opening.len() > PART_BYTES[0] {
            return Err(StatusCode::BAD_REQUEST);
        }
        let total = self.limits.pair_slots.unwrap_or(PAIR_SLOTS);
        let per_space = self.tenants[tenant]
            .limits
            .pair_slots
            .unwrap_or(PAIR_PER_SPACE);
        if total == 0 || per_space == 0 {
            return Err(StatusCode::FORBIDDEN);
        }
        let mut inner = self.inner.lock().unwrap();
        inner.slots.retain(|_, slot| !slot.expired());
        if inner.slots.len() >= total
            || inner.slots.values().filter(|s| s.tenant == tenant).count() >= per_space
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let free: Vec<u16> = (100..=999)
            .filter(|n| !inner.slots.contains_key(n))
            .collect();
        let mut random = [0u8; 2];
        getrandom::getrandom(&mut random).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let number = free[usize::from(u16::from_be_bytes(random)) % free.len()];
        inner.slots.insert(
            number,
            Slot {
                tenant,
                created: Instant::now(),
                parts: [Some(opening), None, None, None],
            },
        );
        Ok(number)
    }
    /// `tenant`: Some for the host side, None for the tokenless client side.
    pub(super) fn write_part(
        &self,
        number: u16,
        index: usize,
        tenant: Option<usize>,
        bytes: Bytes,
    ) -> Answer<()> {
        if bytes.is_empty() || bytes.len() > PART_BYTES[index] {
            return Err(StatusCode::BAD_REQUEST);
        }
        self.with_state(|inner, changed| {
            let slot = inner
                .slots
                .get_mut(&number)
                .filter(|slot| !slot.expired() && tenant.is_none_or(|t| t == slot.tenant))
                .ok_or(StatusCode::NOT_FOUND)?;
            // Each part is written once, and only after the part before it.
            if slot.parts[index].is_some() || slot.parts[index - 1].is_none() {
                return Err(StatusCode::NOT_FOUND);
            }
            slot.parts[index] = Some(bytes);
            changed.push(Wake::Pair(number));
            Ok(())
        })
    }
    pub(super) fn read_part(
        &self,
        number: u16,
        index: usize,
        tenant: Option<usize>,
    ) -> Answer<Option<Bytes>> {
        let inner = self.inner.lock().unwrap();
        let slot = inner
            .slots
            .get(&number)
            .filter(|slot| !slot.expired() && tenant.is_none_or(|t| t == slot.tenant))
            .ok_or(StatusCode::NOT_FOUND)?;
        // The opening is offered only until someone answered it: one attempt per slot.
        if index == 0 && slot.parts[1].is_some() {
            return Err(StatusCode::NOT_FOUND);
        }
        // Reading never removes anything: a lost answer can be read again. The host closes
        // the slot once it has checked the acknowledgement, or the slot expires.
        Ok(slot.parts[index].clone())
    }
    pub(super) fn close_slot(&self, number: u16, tenant: usize) -> Answer<()> {
        self.with_state(|inner, changed| match inner.slots.get(&number) {
            Some(slot) if slot.tenant == tenant => {
                inner.slots.remove(&number);
                changed.push(Wake::Pair(number));
                Ok(())
            }
            _ => Err(StatusCode::NOT_FOUND),
        })
    }
}

pub(super) async fn open_pair(State(relay): State<Arc<Relay>>, request: Request) -> Response {
    let work = async {
        let tenant = relay.authorize(request.headers(), peer(request.extensions()))?;
        let bytes = body(
            request,
            PART_BYTES[0],
            Instant::now() + Duration::from_secs(RECORD_TTL),
        )
        .await?;
        relay.open_slot(tenant, bytes)
    }
    .await;
    match work {
        Ok(number) => {
            let mut response = relay.response(StatusCode::CREATED, Body::from(number.to_string()));
            response
                .headers_mut()
                .insert("content-type", "text/plain".parse().unwrap());
            response
        }
        Err(status) => relay.response(status, Body::empty()),
    }
}

pub(super) async fn close_pair(
    State(relay): State<Arc<Relay>>,
    Path(number): Path<String>,
    Peer(address): Peer,
    headers: HeaderMap,
) -> Response {
    let status = relay
        .authorize(&headers, address)
        .and_then(|tenant| relay.close_slot(number_or_404(&number)?, tenant))
        .err()
        .unwrap_or(StatusCode::NO_CONTENT);
    relay.response(status, Body::empty())
}
fn number_or_404(text: &str) -> Answer<u16> {
    slot_number(text).ok_or(StatusCode::NOT_FOUND)
}

pub(super) async fn write_pair(
    State(relay): State<Arc<Relay>>,
    Path((number, name)): Path<(String, String)>,
    request: Request,
) -> Response {
    let address = peer(request.extensions());
    let Some(index) = part(&name).filter(|&i| i > 0) else {
        return relay.response(StatusCode::NOT_FOUND, Body::empty());
    };
    let work = async {
        let tenant = if tokenless(index, true) {
            relay.open_door(address)?;
            None
        } else {
            Some(relay.authorize(request.headers(), address)?)
        };
        let Some(number) = slot_number(&number) else {
            return Err(refused(&relay, tenant, address));
        };
        let bytes = body(
            request,
            PART_BYTES[index],
            Instant::now() + Duration::from_secs(RECORD_TTL),
        )
        .await?;
        relay
            .write_part(number, index, tenant, bytes)
            .map_err(|status| match status {
                StatusCode::NOT_FOUND => refused(&relay, tenant, address),
                other => other,
            })
    }
    .await;
    relay.response(work.err().unwrap_or(StatusCode::NO_CONTENT), Body::empty())
}
/// Tokenless misses count against the brake; a token holder's miss is a plain 404.
fn refused(relay: &Relay, tenant: Option<usize>, address: Option<IpAddr>) -> Failure {
    match tenant {
        Some(_) => StatusCode::NOT_FOUND,
        None => relay.no_slot(address),
    }
}

pub(super) async fn read_pair(
    State(relay): State<Arc<Relay>>,
    Path((number, name)): Path<(String, String)>,
    Peer(address): Peer,
    headers: HeaderMap,
    query: Result<Query<Wait>, QueryRejection>,
) -> Response {
    let Some(index) = part(&name) else {
        return relay.response(StatusCode::NOT_FOUND, Body::empty());
    };
    let work = async {
        let tenant = if tokenless(index, false) {
            relay.open_door(address)?;
            None
        } else {
            Some(relay.authorize(&headers, address)?)
        };
        let Query(query) = query.map_err(|_| StatusCode::BAD_REQUEST)?;
        if query.wait > LONG_POLL {
            return Err(StatusCode::BAD_REQUEST);
        }
        let Some(number) = slot_number(&number) else {
            return Err(refused(&relay, tenant, address));
        };
        let miss = |status| match status {
            StatusCode::NOT_FOUND => refused(&relay, tenant, address),
            other => other,
        };
        // Tokenless reads never wait, so a stranger who guessed the number cannot hold
        // the client's place; the client polls. The host side may long-poll.
        let wait = if tenant.is_none() { 0 } else { query.wait };
        let deadline = Instant::now() + Duration::from_secs(wait);
        let wake = relay.waiter(Wake::Pair(number));
        loop {
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match relay.read_part(number, index, tenant) {
                Ok(Some(bytes)) => break Ok(Some(bytes)),
                Ok(None) => {}
                Err(status) => break Err(miss(status)),
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break Ok(None);
            }
        }
    }
    .await;
    match work {
        Ok(Some(bytes)) => relay.response(StatusCode::OK, Body::from(bytes)),
        Ok(None) => relay.response(StatusCode::NO_CONTENT, Body::empty()),
        Err(status) => relay.response(status, Body::empty()),
    }
}
