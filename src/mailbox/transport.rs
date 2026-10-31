//! S1–S3 handshake and the selected acknowledged multi-record HTTPS carrier.
use super::http::{Backoff, Failure, Http, packets};
use super::relay::hello_hash;
use super::{BATCH_MS, HEARTBEAT, LONG_POLL, Options, PEER_TIMEOUT, PIPELINE, RECORD_TTL, WINDOW};
use crate::{chain_state::Store, diagnostic::Diagnostic, pairing::PairingRejected, signal::now};
use anyhow::{Result, ensure};
use axum::body::Bytes;
use futures_util::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use k_lani_link_protocol::{
    EndpointRole, Frame, HELLO_BYTES, HelloError, Initiator, MAX_BATCH_FRAMES, MAX_FRAME_BYTES,
    MAX_PACKET_BYTES, REPLY_BYTES, SecureSession, Verdict,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
use tokio_util::sync::CancellationToken;

pub type SharedStore = Arc<Mutex<Store>>;
pub struct HttpsTunnel {
    pub tx: mpsc::Sender<Frame>,
    pub rx: mpsc::Receiver<Frame>,
    pub closed: CancellationToken,
    pub recovering: Arc<AtomicBool>,
    pub rejected: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    #[cfg(test)]
    core: Option<Arc<Core>>,
}
impl HttpsTunnel {
    pub async fn close(&mut self) {
        self.closed.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
        #[cfg(test)]
        self.core.take();
    }
}
impl Drop for HttpsTunnel {
    fn drop(&mut self) {
        self.closed.cancel();
    }
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|_| Diagnostic::err("OS randomness failed"))?;
    Ok(bytes)
}
fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Failure>() == Some(&Failure::Network)
}
/// The hello lifetime the host accepts; a later retry needs a fresh hello.
const HELLO_DEADLINE: Duration = Duration::from_secs(if cfg!(test) { 6 } else { 120 });
/// A delivered hello got no reply because the transport failed. The host may hold it as
/// one of its `MAX_PENDING` attempts, which only a later completed handshake releases.
#[derive(Debug)]
pub struct UnansweredHello;
impl std::fmt::Display for UnansweredHello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("handshake reply lost after the hello reached the relay")
    }
}
fn unanswered(error: anyhow::Error, possible: bool) -> anyhow::Error {
    if possible && retryable(&error) {
        error.context(UnansweredHello)
    } else {
        error
    }
}
fn data_path(room: &str, session: &str, direction: &str) -> String {
    format!("/v1/box/{room}/{session}/{direction}")
}
async fn first(
    http: &Http,
    path: &str,
    stop: &CancellationToken,
    deadline: Instant,
) -> Result<Vec<Bytes>> {
    let mut backoff = Backoff::default();
    loop {
        ensure!(Instant::now() < deadline, Failure::Network);
        let query = format!("{path}?from=1&wait={}", http.wait());
        match http
            .get_bytes(&query, 1, WINDOW * (MAX_PACKET_BYTES + 4), stop, deadline)
            .await
        {
            Ok(Some(body)) => {
                let packets = packets(&body)?;
                if packets.is_empty() {
                    return Err(Failure::Invalid.into());
                }
                return Ok(packets);
            }
            Ok(None) => backoff.reset(),
            Err(error) if retryable(&error) => backoff.wait(&error, stop).await?,
            Err(error) => return Err(error),
        }
    }
}

pub async fn connect(
    options: &Options,
    store: SharedStore,
    stop: CancellationToken,
) -> Result<HttpsTunnel> {
    let http = Http::new(options)?;
    let chain = store.lock().unwrap().client(now())?.chain;
    let attempt = Initiator::start(&chain, random()?, random()?, random()?, now())
        .map_err(|_| PairingRejected)?;
    let room = attempt.room();
    let hash = hello_hash(attempt.hello());
    let hello = Bytes::copy_from_slice(attempt.hello());
    let deadline = Instant::now() + HELLO_DEADLINE;
    let mut backoff = Backoff::default();
    // A host that is merely absent leaves every poll answered with 204. Only a transport
    // failure after delivery can hide a reply the host already saved as pending.
    let (mut delivered, mut interrupted) = (false, false);
    let reply = loop {
        if Instant::now() >= deadline {
            return Err(unanswered(
                Failure::Network.into(),
                delivered && interrupted,
            ));
        }
        if let Err(error) = http
            .put(
                &format!("/v1/hello/{room}/client"),
                0,
                hello.clone(),
                &stop,
                deadline,
            )
            .await
        {
            // The first PUT's answer may be lost after the host already stored the hello.
            let possible = delivered || error.is::<super::http::MaybeDelivered>();
            return Err(unanswered(error, possible));
        }
        delivered = true;
        match http
            .get_bytes(
                &format!("/v1/hello/{room}/host/{hash}?wait={}", http.wait()),
                0,
                REPLY_BYTES,
                &stop,
                deadline,
            )
            .await
        {
            Ok(Some(reply)) => break reply,
            Ok(None) => backoff.reset(),
            Err(error) if retryable(&error) => {
                // Cutting the last long poll at the deadline is not an interruption.
                interrupted |= Instant::now() < deadline;
                backoff.wait(&error, &stop).await?
            }
            Err(error) => return Err(error),
        }
    };
    let mut established = attempt
        .finish(&reply, now())
        .map_err(|e| Diagnostic::err(e.to_string()))?;
    // The successor must survive a crash before the first Ready leaves this process.
    store.lock().unwrap().commit_client(&established, now())?;
    let ready = established
        .session
        .seal_batch(&[Frame::Ready])
        .map_err(|_| PairingRejected)?;
    let source = established.source_room().to_owned();
    let session = established.session_id().to_owned();
    let outbound = data_path(&source, &session, "c2h");
    let inbound = data_path(&source, &session, "h2c");
    let last_sent = Instant::now();
    let ready_deadline = last_sent + Duration::from_secs(PEER_TIMEOUT);
    http.put(
        &format!("{outbound}/1"),
        1,
        Bytes::from(ready),
        &stop,
        ready_deadline,
    )
    .await?;
    let mut initial = first(&http, &inbound, &stop, ready_deadline)
        .await?
        .into_iter();
    let ready = initial.next().ok_or(Failure::Invalid)?;
    let frames = established
        .session
        .open_batch(&ready)
        .map_err(|_| PairingRejected)?;
    ensure!(matches!(frames.as_slice(), [Frame::Ready]), PairingRejected);
    let verified = Instant::now();
    store.lock().unwrap().record_verified_activity(now())?;
    start(
        http,
        store,
        EndpointRole::Client,
        Opening {
            cipher: established.session,
            inbound,
            outbound,
            initial: initial.collect(),
            last_sent,
            verified,
        },
    )
}

type Poll = BoxFuture<'static, (String, Result<Option<Bytes>>)>;
fn poll(
    http: Http,
    room: String,
    publication: Option<(String, Bytes)>,
    stop: CancellationToken,
    slow: bool,
) -> Poll {
    async move {
        let work=async {
            if let Some((hash,reply))=publication {
                // Cached publication is bounded and cannot starve other credential rooms.
                let result=http.put(&format!("/v1/hello/{room}/host/{hash}"),0,reply,&stop,Instant::now()+Duration::from_secs(5)).await;
                if let Err(error)=result
                    && error.downcast_ref::<Failure>()==Some(&Failure::Denied) { return Err(error); }
            }
            if slow {
                tokio::select! { _=stop.cancelled()=>return Err(Failure::Network.into()), _=tokio::time::sleep(Duration::from_millis(500))=>{} }
            }
            http.get_bytes(&format!("/v1/hello/{room}/client?wait={}",http.wait()),0,HELLO_BYTES,&stop,Instant::now()+Duration::from_secs(LONG_POLL+10)).await
        }.await;
        (room,work)
    }.boxed()
}
pub async fn accept(
    options: &Options,
    store: SharedStore,
    stop: CancellationToken,
) -> Result<HttpsTunnel> {
    let http = Http::new(options)?;
    let rooms = store.lock().unwrap().host(now())?.chains.rooms();
    let mut polls = FuturesUnordered::new();
    for room in rooms {
        polls.push(poll(http.clone(), room, None, stop.clone(), false));
    }
    let mut cached: HashMap<String, (String, Bytes)> = HashMap::new();
    loop {
        let item = tokio::select! { _=stop.cancelled()=>return Err(Failure::Network.into()), item=polls.next()=>item };
        let (room, result) = item.ok_or(Failure::Network)?;
        let hello = match result {
            Ok(Some(hello)) => hello,
            Ok(None) => {
                polls.push(poll(http.clone(), room, None, stop.clone(), false));
                continue;
            }
            Err(error) if retryable(&error) => {
                polls.push(poll(http.clone(), room, None, stop.clone(), true));
                continue;
            }
            Err(error) => return Err(error),
        };
        let hash = hello_hash(&hello);
        if let Some((previous, reply)) = cached.get(&room)
            && previous == &hash
        {
            polls.push(poll(
                http.clone(),
                room.clone(),
                Some((hash, reply.clone())),
                stop.clone(),
                true,
            ));
            continue;
        }
        let verdict =
            match store
                .lock()
                .unwrap()
                .accept_host(&room, &hello, random()?, random()?, now())
            {
                Ok(verdict) => verdict,
                Err(error)
                    if crate::diagnostic::printable(&error)
                        == Some(HelloError::PendingFull.to_string()) =>
                {
                    polls.push(poll(http.clone(), room, None, stop.clone(), true));
                    continue;
                }
                Err(error) => return Err(error),
            };
        let mut accepted = match verdict {
            Verdict::Accepted(accepted) => accepted,
            Verdict::CachedReply(reply) | Verdict::Stale(reply) => {
                let reply = Bytes::from(reply);
                cached.insert(room.clone(), (hash.clone(), reply.clone()));
                polls.push(poll(
                    http.clone(),
                    room,
                    Some((hash, reply)),
                    stop.clone(),
                    true,
                ));
                continue;
            }
            Verdict::Ignored => {
                polls.push(poll(http.clone(), room, None, stop.clone(), true));
                continue;
            }
        };
        // accept_host already durably saved this exact pending reply.
        http.put(
            &format!("/v1/hello/{room}/host/{hash}"),
            0,
            Bytes::copy_from_slice(accepted.reply()),
            &stop,
            Instant::now() + Duration::from_secs(PEER_TIMEOUT),
        )
        .await?;
        let source = accepted.source_room().to_owned();
        let session = accepted.session_id().to_owned();
        let inbound = data_path(&source, &session, "c2h");
        let outbound = data_path(&source, &session, "h2c");
        let initial = first(
            &http,
            &inbound,
            &stop,
            Instant::now() + Duration::from_secs(PEER_TIMEOUT),
        )
        .await?;
        ensure!(initial.len() == 1, PairingRejected);
        let verified = Instant::now();
        // This verifies the first [Ready] and saves confirmation before any host Ready or app frame.
        store
            .lock()
            .unwrap()
            .confirm_host(&mut accepted, &initial[0], now())?;
        let ready = accepted
            .session
            .seal_batch(&[Frame::Ready])
            .map_err(|_| PairingRejected)?;
        let last_sent = Instant::now();
        http.put(
            &format!("{outbound}/1"),
            1,
            Bytes::from(ready),
            &stop,
            Instant::now() + Duration::from_secs(PEER_TIMEOUT),
        )
        .await?;
        return start(
            http,
            store,
            EndpointRole::Host,
            Opening {
                cipher: accepted.session,
                inbound,
                outbound,
                initial: VecDeque::new(),
                last_sent,
                verified,
            },
        );
    }
}

struct Activity {
    verified: Instant,
    wall: u64,
    saved: u64,
    seen: u64,
    expires: u64,
    save_after: Instant,
}
fn lifecycle(store: &Store, role: EndpointRole) -> Result<crate::chain_state::Lifecycle> {
    Ok(match role {
        EndpointRole::Client => store.client(now())?.lifecycle,
        EndpointRole::Host => store.host(now())?.lifecycle,
    })
}
impl Activity {
    fn new(store: &Store, role: EndpointRole, verified: Instant) -> Result<Self> {
        let life = lifecycle(store, role)?;
        Ok(Self {
            verified,
            wall: life.last_activity(),
            saved: life.last_activity(),
            seen: life.last_seen(),
            expires: life.expires_at(),
            save_after: Instant::now()
                + Duration::from_secs(60.min(life.expires_at().saturating_sub(now()) / 2).max(1)),
        })
    }
    fn record(&mut self, store: &SharedStore, role: EndpointRole) -> Result<()> {
        let clock = now();
        ensure!(clock >= self.seen && clock < self.expires, Failure::Denied);
        if Instant::now() >= self.save_after && clock > self.saved {
            let mut store = store.lock().unwrap();
            store.record_verified_activity(clock)?;
            let life = lifecycle(&store, role)?;
            self.saved = life.last_activity();
            self.seen = life.last_seen();
            self.expires = life.expires_at();
            self.save_after = Instant::now()
                + Duration::from_secs(60.min(self.expires.saturating_sub(clock) / 2).max(1));
        }
        self.wall = clock;
        self.verified = Instant::now();
        Ok(())
    }
}
struct Core {
    http: Http,
    store: SharedStore,
    role: EndpointRole,
    cipher: Mutex<SecureSession>,
    activity: Mutex<Activity>,
    closed: CancellationToken,
    recovering: Arc<AtomicBool>,
}
struct Opening {
    cipher: SecureSession,
    inbound: String,
    outbound: String,
    initial: VecDeque<Bytes>,
    last_sent: Instant,
    verified: Instant,
}
fn start(
    http: Http,
    store: SharedStore,
    role: EndpointRole,
    opening: Opening,
) -> Result<HttpsTunnel> {
    let Opening {
        cipher,
        inbound,
        outbound,
        initial,
        last_sent,
        verified,
    } = opening;
    let activity = Activity::new(&store.lock().unwrap(), role, verified)?;
    let closed = CancellationToken::new();
    let recovering = Arc::new(AtomicBool::new(false));
    let rejected = Arc::new(AtomicBool::new(false));
    let core = Arc::new(Core {
        http,
        store,
        role,
        cipher: Mutex::new(cipher),
        activity: Mutex::new(activity),
        closed: closed.clone(),
        recovering: recovering.clone(),
    });
    let (tx, rx) = mpsc::channel(32);
    let (input, incoming) = mpsc::channel(32);
    let failure = rejected.clone();
    #[cfg(test)]
    let inspect = core.clone();
    let worker = tokio::spawn(async move {
        let result = tokio::select! {
            _=core.closed.cancelled()=>Ok(()),
            result=read(core.clone(),inbound,initial,input)=>result,
            result=write(core.clone(),outbound,rx,last_sent)=>result,
            result=watch(core.clone())=>result,
        };
        if result.as_ref().is_err_and(|e| {
            !matches!(
                e.downcast_ref::<Failure>(),
                Some(Failure::Network | Failure::Gone)
            )
        }) {
            failure.store(true, Ordering::SeqCst);
        }
        core.closed.cancel();
        let activity = core.activity.lock().unwrap();
        // Checkpoint only authenticated activity, at close or at most once per minute, not per token.
        if activity.wall > activity.saved
            && core
                .store
                .lock()
                .unwrap()
                .record_verified_activity(activity.wall)
                .is_err()
        {
            failure.store(true, Ordering::SeqCst);
        }
    });
    Ok(HttpsTunnel {
        tx,
        rx: incoming,
        closed,
        recovering,
        rejected,
        worker: Some(worker),
        #[cfg(test)]
        core: Some(inspect),
    })
}
async fn watch(core: Arc<Core>) -> Result<()> {
    loop {
        let deadline = core.activity.lock().unwrap().verified + Duration::from_secs(PEER_TIMEOUT);
        tokio::time::sleep_until(deadline.min(Instant::now() + Duration::from_secs(1))).await;
        let activity = core.activity.lock().unwrap();
        ensure!(
            Instant::now() < activity.verified + Duration::from_secs(PEER_TIMEOUT),
            Failure::Gone
        );
        ensure!(
            now() >= activity.seen && now() < activity.expires,
            Failure::Denied
        );
    }
}
async fn deliver(
    core: &Core,
    packet: &[u8],
    cursor: &mut u64,
    input: &mpsc::Sender<Frame>,
) -> Result<()> {
    ensure!(
        packet.len() >= 28 && &packet[..4] == b"KLI2",
        PairingRejected
    );
    ensure!(
        u64::from_be_bytes(packet[4..12].try_into().unwrap()) == *cursor - 1,
        PairingRejected
    );
    let frames = core
        .cipher
        .lock()
        .unwrap()
        .open_batch(packet)
        .map_err(|_| PairingRejected)?;
    *cursor = cursor.checked_add(1).ok_or(Failure::Invalid)?;
    core.activity
        .lock()
        .unwrap()
        .record(&core.store, core.role)?;
    core.recovering.store(false, Ordering::SeqCst);
    for frame in frames {
        if !matches!(frame, Frame::Ready) {
            input.send(frame).await.map_err(|_| Failure::Network)?;
        }
    }
    Ok(())
}
async fn read(
    core: Arc<Core>,
    path: String,
    mut initial: VecDeque<Bytes>,
    input: mpsc::Sender<Frame>,
) -> Result<()> {
    let mut cursor = core
        .cipher
        .lock()
        .unwrap()
        .receive_sequence()
        .checked_add(1)
        .ok_or(Failure::Invalid)?;
    while let Some(packet) = initial.pop_front() {
        deliver(&core, &packet, &mut cursor, &input).await?;
    }
    let mut backoff = Backoff::default();
    loop {
        let result = async {
            let started = Instant::now();
            let mut response = core
                .http
                .get(
                    &format!("{path}?from={cursor}&wait={}", core.http.wait()),
                    cursor,
                    &core.closed,
                    Instant::now() + Duration::from_secs(LONG_POLL + 10),
                )
                .await?;
            if response.status() == 204 {
                core.http.idle(&core.closed, started).await?;
                return Ok(());
            }
            core.http.active();
            let mut buffer = bytes::BytesMut::new();
            let mut total = 0usize;
            let mut count = 0;
            while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Network)? {
                total = total.checked_add(chunk.len()).ok_or(Failure::Invalid)?;
                ensure!(total <= WINDOW * (MAX_PACKET_BYTES + 4), Failure::Invalid);
                buffer.extend_from_slice(&chunk);
                loop {
                    if buffer.len() < 4 {
                        break;
                    }
                    let length = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
                    ensure!(
                        (28..=MAX_PACKET_BYTES).contains(&length) && count < WINDOW,
                        Failure::Invalid
                    );
                    if buffer.len() < 4 + length {
                        break;
                    }
                    let packet = buffer.split_to(4 + length).freeze();
                    deliver(&core, &packet[4..], &mut cursor, &input).await?;
                    count += 1;
                }
            }
            // A cut response retains the cursor at the last authenticated prefix.
            ensure!(buffer.is_empty(), Failure::Network);
            ensure!(count > 0, Failure::Invalid);
            Ok::<_, anyhow::Error>(())
        }
        .await;
        match result {
            Ok(()) => backoff.reset(),
            Err(error) if retryable(&error) => {
                core.recovering.store(true, Ordering::SeqCst);
                backoff.wait(&error, &core.closed).await?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests;
async fn write(
    core: Arc<Core>,
    path: String,
    input: mpsc::Receiver<Frame>,
    last_sent: Instant,
) -> Result<()> {
    let (queued, packets) = mpsc::channel::<(u64, Bytes)>(PIPELINE);
    tokio::try_join!(
        produce(core.clone(), input, queued, last_sent),
        publish(core, path, packets)
    )?;
    Ok(())
}
async fn produce(
    core: Arc<Core>,
    mut input: mpsc::Receiver<Frame>,
    queued: mpsc::Sender<(u64, Bytes)>,
    mut last_sent: Instant,
) -> Result<()> {
    let mut carry = None;
    loop {
        let heartbeat = last_sent + Duration::from_secs(HEARTBEAT);
        let first = if let Some(frame) = carry.take() {
            Some(frame)
        } else {
            tokio::select! {
                frame=input.recv()=>Some(frame.ok_or(Failure::Network)?),
                _=tokio::time::sleep_until(heartbeat)=>None,
            }
        };
        let mut frames = Vec::new();
        if let Some(first) = first {
            ensure!(!matches!(first, Frame::Ready), Failure::Invalid);
            frames.push(first);
            let deadline = (Instant::now() + Duration::from_millis(BATCH_MS)).min(heartbeat);
            while frames.len() < MAX_BATCH_FRAMES {
                let next = match input.try_recv() {
                    Ok(frame) => Some(frame),
                    Err(mpsc::error::TryRecvError::Empty) => {
                        tokio::time::timeout_at(deadline, input.recv())
                            .await
                            .ok()
                            .flatten()
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => None,
                };
                let Some(next) = next else { break };
                ensure!(!matches!(next, Frame::Ready), Failure::Invalid);
                frames.push(next);
                if serde_json::to_vec(&frames)?.len() > MAX_FRAME_BYTES {
                    carry = frames.pop();
                    break;
                }
            }
        } else {
            frames.push(Frame::Ready);
        }
        let (sequence, bytes) = {
            let mut cipher = core.cipher.lock().unwrap();
            let sequence = cipher
                .send_sequence()
                .checked_add(1)
                .ok_or(Failure::Invalid)?;
            let bytes = cipher.seal_batch(&frames).map_err(|_| Failure::Invalid)?;
            (sequence, Bytes::from(bytes))
        };
        last_sent = Instant::now();
        queued
            .send((sequence, bytes))
            .await
            .map_err(|_| Failure::Network)?;
    }
}
async fn publish(
    core: Arc<Core>,
    path: String,
    mut packets: mpsc::Receiver<(u64, Bytes)>,
) -> Result<()> {
    let mut pending: FuturesUnordered<BoxFuture<'static, Result<()>>> = FuturesUnordered::new();
    loop {
        tokio::select! {
            result=pending.next(),if !pending.is_empty()=>{ result.ok_or(Failure::Network)??; }
            item=packets.recv(),if pending.len()<core.http.writes()=>{
                let (sequence,bytes)=item.ok_or(Failure::Network)?;
                let http=core.http.clone();let stop=core.closed.clone();let target=format!("{path}/{sequence}");
                pending.push(async move { http.put(&target,sequence,bytes,&stop,Instant::now()+Duration::from_secs(RECORD_TTL)).await }.boxed());
            }
        }
    }
}
