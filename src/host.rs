use crate::{config::Config, peer, routing::Routes, tunnel::Tunnel};
use anyhow::{Result, ensure};
use futures_util::StreamExt;
use k_lani_link_protocol::{
    ANTHROPIC_VERSION, Api, CHUNK_BYTES, Frame, MAX_BODY_BYTES, MAX_CONTROL_BODY_BYTES,
    RESPONSE_WINDOW, UPLOAD_SECONDS, allowed_route, control_body_valid, decode_data,
    management_route, route_api,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

struct Upload {
    id: u64,
    method: String,
    path: String,
    body: Vec<u8>,
    started: Instant,
    control: bool,
}
struct Job {
    serial: u64,
    control: bool,
    cancel: CancellationToken,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
struct Outlet {
    serial: u64,
    /// The session this response is delivered to; `None` while detached.
    session: Option<u64>,
    control: mpsc::UnboundedSender<Control>,
    /// The job has ended; the outlet only keeps its tail for a resumed session.
    finished: Arc<std::sync::atomic::AtomicBool>,
}
/// Finished responses kept for a possible resume. They no longer hold a request slot, so
/// they need their own bound; the oldest go first.
const LINGERING: usize = 16;
enum Control {
    Credit,
    Attach {
        tx: mpsc::Sender<Frame>,
        response: bool,
        received: u64,
    },
    Detach,
}
/// Running jobs and the outlets of their responses. HTTPS keeps one registry across
/// sessions when resume is enabled; otherwise it lives exactly as long as one session.
struct Registry {
    jobs: HashMap<u64, Job>,
    outlets: HashMap<u64, Outlet>,
    serial: u64,
    session: u64,
    finished: mpsc::UnboundedSender<(u64, u64)>,
    done: mpsc::UnboundedReceiver<(u64, u64)>,
    released: mpsc::UnboundedSender<(u64, u64)>,
    gone: mpsc::UnboundedReceiver<(u64, u64)>,
}
impl Default for Registry {
    fn default() -> Self {
        let (finished, done) = mpsc::unbounded_channel();
        let (released, gone) = mpsc::unbounded_channel();
        Self {
            jobs: HashMap::new(),
            outlets: HashMap::new(),
            serial: 0,
            session: 0,
            finished,
            done,
            released,
            gone,
        }
    }
}
impl Registry {
    /// The session ended. Without resume every job stops now. Otherwise each outlet keeps
    /// its response for `resume` and cancels the job if no later session resumes it.
    fn detach(&mut self, resume: Duration) {
        if resume.is_zero() {
            *self = Self::default();
            return;
        }
        for outlet in self.outlets.values_mut() {
            outlet.session = None;
            let _ = outlet.control.send(Control::Detach);
        }
    }
    fn trim_lingering(&mut self) {
        let mut finished: Vec<(u64, u64)> = self
            .outlets
            .iter()
            .filter(|(_, outlet)| outlet.finished.load(std::sync::atomic::Ordering::SeqCst))
            .map(|(id, outlet)| (outlet.serial, *id))
            .collect();
        if finished.len() <= LINGERING {
            return;
        }
        finished.sort_unstable();
        // Dropping the control channel ends the outlet task; the job is already done.
        for (_, id) in &finished[..finished.len() - LINGERING] {
            self.outlets.remove(id);
        }
    }
    /// A detached response from an earlier gateway process may still hold this ID.
    fn evict_detached(&mut self, id: u64) {
        if self
            .outlets
            .get(&id)
            .is_some_and(|outlet| outlet.session != Some(self.session))
        {
            self.outlets.remove(&id);
            self.jobs.remove(&id);
        }
    }
}

/// Carries one job's frames to the current session and keeps what a resumed session may
/// still need: the Response, every uncredited Data frame and the terminal frame. Credits
/// bound the Data frames, so a detached response holds at most one window.
fn outlet(
    id: u64,
    serial: u64,
    credits: Arc<Semaphore>,
    cancel: CancellationToken,
    tx: mpsc::Sender<Frame>,
    resume: Duration,
    released: mpsc::UnboundedSender<(u64, u64)>,
) -> (
    mpsc::Sender<Frame>,
    mpsc::UnboundedSender<Control>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let (input, mut frames) = mpsc::channel(RESPONSE_WINDOW + 4);
    let (control, mut controls) = mpsc::unbounded_channel();
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ended = finished.clone();
    tokio::spawn(async move {
        let mut tx = Some(tx);
        let mut response = None;
        let mut unacked = VecDeque::new();
        let mut base = 0u64;
        let mut terminal = None;
        let mut producing = true;
        let mut delivered = false;
        let mut expiry: Option<tokio::time::Instant> = None;
        loop {
            tokio::select! {
                frame = frames.recv(), if producing => match frame {
                    None => {
                        producing = false;
                        if terminal.is_none() { break; }
                    }
                    Some(frame) => {
                        match &frame {
                            Frame::Response { .. } => response = Some(frame.clone()),
                            Frame::Data { .. } => unacked.push_back(frame.clone()),
                            Frame::End { .. } | Frame::Error { .. } => {
                                terminal = Some(frame.clone());
                                ended.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            _ => {}
                        }
                        let last = terminal.is_some();
                        deliver(&mut tx, frame).await;
                        if last && tx.is_some() {
                            delivered = true;
                            if resume.is_zero() { break; }
                            // The terminal frame may still die with this session.
                            expiry = Some(tokio::time::Instant::now() + resume);
                        }
                    }
                },
                control = controls.recv() => match control {
                    None => break,
                    Some(Control::Credit) => {
                        if unacked.pop_front().is_some() {
                            base += 1;
                            credits.add_permits(1);
                        }
                    }
                    Some(Control::Detach) => {
                        tx = None;
                        delivered = false;
                        expiry = Some(tokio::time::Instant::now() + resume);
                    }
                    Some(Control::Attach { tx: next, response: seen, received }) => {
                        let held = base + unacked.len() as u64;
                        if received < base || received > held || !seen && received != 0 {
                            let _ = next.send(Frame::Error { id, message: "response cannot be resumed".into() }).await;
                            break;
                        }
                        // These reached the caller; their credits died with the old session.
                        let skipped = (received - base) as usize;
                        unacked.drain(..skipped);
                        base = received;
                        credits.add_permits(skipped);
                        tx = Some(next);
                        expiry = None;
                        let replay: Vec<Frame> = (!seen)
                            .then(|| response.clone())
                            .flatten()
                            .into_iter()
                            .chain(unacked.iter().cloned())
                            .chain(terminal.clone())
                            .collect();
                        for frame in replay {
                            deliver(&mut tx, frame).await;
                        }
                        delivered = terminal.is_some() && tx.is_some();
                        if delivered {
                            expiry = Some(tokio::time::Instant::now() + resume);
                        }
                    }
                },
                _ = tokio::time::sleep_until(expiry.unwrap_or_else(tokio::time::Instant::now)), if expiry.is_some() => break,
            }
        }
        // Nobody resumed in time: stop the backend instead of generating unread.
        if !delivered {
            cancel.cancel();
        }
        let _ = released.send((id, serial));
    });
    (input, control, finished)
}
async fn deliver(tx: &mut Option<mpsc::Sender<Frame>>, frame: Frame) {
    if let Some(sender) = tx
        && sender.send(frame).await.is_err()
    {
        *tx = None;
    }
}

pub async fn run(
    config: Config,
    stop: CancellationToken,
    state_dir: std::path::PathBuf,
) -> Result<()> {
    if config.transport == crate::config::Transport::Https {
        return run_https(&config, stop).await;
    }
    let mut lease = crate::pairing::PairingLease::reserve(&state_dir, &config.pair_key)?;
    let mut previous = String::new();
    loop {
        let connected = tokio::select! { _ = stop.cancelled() => return Ok(()), p = peer::accept(&config, &mut previous) => p };
        match connected {
            Ok(peer) => {
                lease.consume()?;
                return tokio::select! { _ = stop.cancelled() => Ok(()), result = serve(&config, peer) => result };
            }
            Err(error) if error.is::<crate::pairing::PairingRejected>() => return Err(error),
            Err(_) => {}
        }
        tokio::select! { _ = stop.cancelled() => return Ok(()), _ = tokio::time::sleep(Duration::from_secs(2)) => {} }
    }
}

async fn run_https(config: &Config, stop: CancellationToken) -> Result<()> {
    let options = config
        .https
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("HTTPS settings required"))?;
    let store = Arc::new(std::sync::Mutex::new(crate::chain_state::Store::open(
        &options.state_dir,
        k_lani_link_protocol::EndpointRole::Host,
    )?));
    store.lock().unwrap().host(crate::signal::now())?;
    let mut registry = Registry::default();
    let resume = Duration::from_secs(config.resume_seconds);
    loop {
        let result = tokio::select! {
            biased;
            _=stop.cancelled()=>return Ok(()),
            result=crate::mailbox::transport::accept(options,store.clone(),stop.clone())=>result,
        };
        match result {
            Ok(peer) => {
                let rejected = peer.rejected.clone();
                tokio::select! {
                    biased;
                    _=stop.cancelled()=>return Ok(()),
                    result=serve_with(config,peer.into(),&mut registry)=>result?,
                }
                registry.detach(resume);
                if rejected.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(crate::diagnostic::Diagnostic::err(
                        "HTTPS session rejected; check the peer and pairing state",
                    ));
                }
                // Listen for the next client handshake. A job never runs twice; with resume
                // enabled its unconfirmed response frames may be delivered again.
            }
            Err(error)
                if matches!(
                    error.downcast_ref::<crate::mailbox::http::Failure>(),
                    Some(
                        crate::mailbox::http::Failure::Network
                            | crate::mailbox::http::Failure::Gone
                    )
                ) => {}
            Err(error) => return Err(error),
        }
        tokio::select! { _=stop.cancelled()=>return Ok(()), _=tokio::time::sleep(Duration::from_millis(250))=>{} }
    }
}

pub async fn serve(config: &Config, peer: impl Into<Tunnel>) -> Result<()> {
    // Dropping the registry with the session cancels every job it still holds.
    serve_with(config, peer.into(), &mut Registry::default()).await
}

async fn serve_with(config: &Config, mut peer: Tunnel, registry: &mut Registry) -> Result<()> {
    let closed = peer.closed().clone();
    config.validate()?;
    let routes = Arc::new(Routes::new(config)?);
    let mut uploads = HashMap::<u64, Upload>::new();
    let mut queue = VecDeque::<Upload>::new();
    registry.session += 1;
    let resume = Duration::from_secs(config.resume_seconds);
    let mut deadline_tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        // FIFO admission: a management operation drains earlier requests and prevents
        // later ones from entering the backend until the operation has completed.
        while let Some(front) = queue.front() {
            if registry.jobs.values().any(|j| j.control)
                || (front.control && !registry.jobs.is_empty())
                || registry.jobs.len() >= config.max_concurrent_requests
            {
                break;
            }
            let upload = queue.pop_front().unwrap();
            let id = upload.id;
            let control = upload.control;
            let cancel = CancellationToken::new();
            let credits = Arc::new(Semaphore::new(RESPONSE_WINDOW));
            registry.serial += 1;
            let job_serial = registry.serial;
            registry.jobs.insert(
                id,
                Job {
                    serial: job_serial,
                    control,
                    cancel: cancel.clone(),
                },
            );
            let (tx, outlet_control, outlet_finished) = outlet(
                id,
                job_serial,
                credits.clone(),
                cancel.clone(),
                peer.tx().clone(),
                resume,
                registry.released.clone(),
            );
            registry.outlets.insert(
                id,
                Outlet {
                    serial: job_serial,
                    session: Some(registry.session),
                    control: outlet_control,
                    finished: outlet_finished,
                },
            );
            registry.trim_lingering();
            let cfg = config.clone();
            let routes = routes.clone();
            let finished = registry.finished.clone();
            tokio::spawn(async move {
                let deadline = tokio::time::Instant::from_std(
                    upload.started + Duration::from_secs(cfg.request_timeout_seconds),
                );
                let work = async {
                    if control {
                        let result = forward_control(&cfg, upload, &tx, credits).await;
                        routes.invalidate();
                        result
                    } else {
                        forward(&routes, upload, &tx, credits).await
                    }
                };
                tokio::select! {
                    _ = cancel.cancelled() => {},
                    result = tokio::time::timeout_at(deadline, work) => {
                        let message = match result {
                            Ok(Ok(())) => None,
                            Ok(Err(_)) => Some("upstream failed"),
                            Err(_) => Some("request timed out"),
                        };
                        if let Some(message) = message {
                            let _ = tx.send(Frame::Error { id, message: message.into() }).await;
                        }
                    }
                }
                cancel.cancel();
                let _ = finished.send((id, job_serial));
            });
        }
        tokio::select! {
            _ = closed.cancelled() => break,
            Some((id, serial)) = registry.done.recv() => {
                if registry.jobs.get(&id).is_some_and(|j| j.serial == serial) { registry.jobs.remove(&id); }
            }
            Some((id, serial)) = registry.gone.recv() => {
                if registry.outlets.get(&id).is_some_and(|o| o.serial == serial) { registry.outlets.remove(&id); }
            }
            _ = deadline_tick.tick() => {
                let expired: Vec<_> = uploads.iter().filter(|(_,u)| u.started.elapsed() > Duration::from_secs(UPLOAD_SECONDS)).map(|(id,_)| *id).collect();
                for id in expired {
                    uploads.remove(&id);
                    reject(peer.tx(), id, 408, "request upload timed out").await?;
                }
                let mut waiting = VecDeque::new();
                while let Some(upload) = queue.pop_front() {
                    if upload.started.elapsed() >= Duration::from_secs(config.request_timeout_seconds) {
                        reject(peer.tx(), upload.id, 504, "request expired in queue").await?;
                    } else { waiting.push_back(upload); }
                }
                queue = waiting;
            }
            frame = peer.receive() => {
                let Some(frame) = frame else { break };
                let id = frame.id();
                match frame {
                    Frame::Request { method, path, .. } => {
                        registry.evict_detached(id);
                        ensure!(id != 0 && !uploads.contains_key(&id) && !registry.jobs.contains_key(&id)
                            && !registry.outlets.contains_key(&id)
                            && !queue.iter().any(|u| u.id == id), "duplicate request ID");
                        let control = management_route(&method, &path);
                        let occupied = uploads.values().filter(|u| u.control == control).count()
                            + queue.iter().filter(|u| u.control == control).count()
                            + registry.jobs.values().filter(|j| j.control == control).count();
                        let capacity = if control { 1 } else { config.max_concurrent_requests + config.max_queued_requests };
                        if !(allowed_route(&method, &path) || control && config.model_control_socket.is_some()) {
                            reject(peer.tx(), id, 404, "route not available").await?;
                        } else if occupied >= capacity {
                            reject(peer.tx(), id, 429, "request capacity exhausted").await?;
                        } else {
                            uploads.insert(id, Upload { id, method, path, body: vec![], started: Instant::now(), control });
                        }
                    }
                    Frame::Data { data, .. } => {
                        if let Some(upload) = uploads.get_mut(&id) {
                            let data = decode_data(&data).map_err(anyhow::Error::msg)?;
                            let limit = if upload.control { MAX_CONTROL_BODY_BYTES } else { MAX_BODY_BYTES };
                            ensure!(upload.body.len() + data.len() <= limit, "request exceeds size limit");
                            upload.body.extend_from_slice(&data);
                        }
                    }
                    Frame::RequestEnd { .. } => {
                        if let Some(upload) = uploads.remove(&id) {
                            if upload.control && !control_body_valid(&upload.method, &upload.body) {
                                reject(peer.tx(), id, 400, "invalid model control command").await?;
                            } else { queue.push_back(upload); }
                        }
                    }
                    Frame::Credit { .. } => {
                        if let Some(outlet) = registry.outlets.get(&id)
                            && outlet.session == Some(registry.session) { let _ = outlet.control.send(Control::Credit); }
                    }
                    Frame::Resume { response, received, .. } => {
                        let session = registry.session;
                        let attached = match registry.outlets.get_mut(&id) {
                            Some(outlet) if !resume.is_zero() && outlet.session.is_none() => {
                                outlet.session = Some(session);
                                outlet.control.send(Control::Attach { tx: peer.tx().clone(), response, received }).is_ok()
                            }
                            _ => false,
                        };
                        if !attached {
                            peer.tx().send(Frame::Error { id, message: "response cannot be resumed".into() }).await?;
                        }
                    }
                    Frame::Cancel { .. } => {
                        uploads.remove(&id);
                        queue.retain(|u| u.id != id);
                        // Once a model change starts, finish it even if its caller leaves.
                        // Otherwise later inference could race a half-finished reload.
                        if let Some(job) = registry.jobs.get(&id)
                            && !job.control {
                            // Keep the scheduling slot until the forwarding future has
                            // actually dropped its upstream response and reports done.
                            job.cancel.cancel();
                        }
                    }
                    _ => anyhow::bail!("unexpected frame from API client"),
                }
            }
        }
    }
    peer.close().await;
    Ok(())
}

async fn reject(tx: &mpsc::Sender<Frame>, id: u64, status: u16, message: &str) -> Result<()> {
    tx.send(Frame::Response {
        id,
        status,
        content_type: "application/json".into(),
    })
    .await?;
    tx.send(Frame::data(
        id,
        &serde_json::to_vec(&serde_json::json!({"error":{"message":message,"code":status}}))?,
    ))
    .await?;
    tx.send(Frame::End { id }).await?;
    Ok(())
}

#[cfg(unix)]
async fn forward_control(
    config: &Config,
    upload: Upload,
    tx: &mpsc::Sender<Frame>,
    credits: Arc<Semaphore>,
) -> Result<()> {
    let path = config
        .model_control_socket
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("model management disabled"))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none());
    let client = client.unix_socket(path.as_path());
    let client = client.build()?;
    let method = reqwest::Method::from_bytes(upload.method.as_bytes())?;
    let mut response = client
        .request(method, format!("http://localhost{}", upload.path))
        .header("content-type", "application/json")
        .body(upload.body)
        .send()
        .await?;
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= 64 * 1024,
            "control response too large"
        );
        body.extend_from_slice(&chunk);
    }
    respond(tx, credits, upload.id, status, &body).await
}

/// A complete JSON response, sent within the credit window.
async fn respond(
    tx: &mpsc::Sender<Frame>,
    credits: Arc<Semaphore>,
    id: u64,
    status: u16,
    body: &[u8],
) -> Result<()> {
    tx.send(Frame::Response {
        id,
        status,
        content_type: "application/json".into(),
    })
    .await?;
    for bytes in body.chunks(CHUNK_BYTES) {
        credits.acquire().await?.forget();
        tx.send(Frame::data(id, bytes)).await?;
    }
    tx.send(Frame::End { id }).await?;
    Ok(())
}

#[cfg(not(unix))]
async fn forward_control(
    _config: &Config,
    _upload: Upload,
    _tx: &mpsc::Sender<Frame>,
    _credits: Arc<Semaphore>,
) -> Result<()> {
    anyhow::bail!("model management needs Unix")
}

async fn forward(
    routes: &Routes,
    upload: Upload,
    tx: &mpsc::Sender<Frame>,
    credits: Arc<Semaphore>,
) -> Result<()> {
    let id = upload.id;
    if routes.is_routed() && upload.method == "GET" && upload.path == "/v1/models" {
        let body = routes.model_list().await?;
        return respond(tx, credits, id, 200, &body).await;
    }
    let target = routes.target(routes.choose(&upload.body).await);
    // Recheck before each request: a restarted backend must not silently resume logging.
    // This is a configuration interlock, not remote attestation of arbitrary software.
    if target.verify {
        crate::backend::verify_at(target.http, target.upstream, target.api_key).await?;
    }
    let anthropic = route_api(&upload.method, &upload.path) == Some(Api::Anthropic);
    let method = reqwest::Method::from_bytes(upload.method.as_bytes())?;
    let url = format!("{}{}", target.upstream.trim_end_matches('/'), upload.path);
    let mut request = target
        .http
        .request(method, url)
        .header("content-type", "application/json")
        .body(upload.body);
    if !target.api_key.is_empty() {
        request = request.bearer_auth(target.api_key);
    }
    // The caller's own version and beta headers stay local: the host pins the version, so
    // a client update cannot change what the backend is asked for.
    if anthropic {
        request = request.header("anthropic-version", ANTHROPIC_VERSION);
        if !target.api_key.is_empty() {
            request = request.header("x-api-key", target.api_key);
        }
    }
    let response = request.send().await?;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_owned();
    tx.send(Frame::Response {
        id,
        status,
        content_type,
    })
    .await?;
    let mut body = response.bytes_stream();
    // While no credit is free, coalesce small upstream items (one SSE event each) into
    // the next chunk. The window then limits bytes, not events; nothing waits on a timer.
    let mut pending = bytes::BytesMut::new();
    let mut ended = false;
    loop {
        if ended && pending.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            permit = credits.acquire(), if !pending.is_empty() => {
                permit?.forget();
                let bytes = pending.split_to(pending.len().min(CHUNK_BYTES));
                tx.send(Frame::data(id, &bytes)).await?;
            }
            item = body.next(), if !ended && pending.len() < CHUNK_BYTES => match item {
                Some(bytes) => pending.extend_from_slice(&bytes?),
                None => ended = true,
            },
        }
    }
    tx.send(Frame::End { id }).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k_lani_link_protocol::MAX_FRAME_BYTES;

    #[test]
    fn a_full_response_chunk_fits_one_sealed_frame() {
        let frames = [Frame::data(u64::MAX, &[0xff; CHUNK_BYTES])];
        assert!(serde_json::to_vec(&frames).unwrap().len() <= MAX_FRAME_BYTES);
    }

    #[tokio::test]
    async fn withheld_credits_coalesce_small_events_without_reordering_or_oversized_chunks() {
        let events: Vec<_> = (0..2000)
            .map(|n| format!("data: {{\"n\":{n}}}\n\n").into_bytes())
            .chain([vec![b'x'; 3 * CHUNK_BYTES]])
            .collect();
        let expected = events.concat();
        let router = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(move || {
                let events = events.clone();
                async move {
                    axum::body::Body::from_stream(futures_util::stream::iter(
                        events
                            .into_iter()
                            .map(|e| Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(e))),
                    ))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let root = std::env::temp_dir();
        let mut config = crate::mailbox::setup::config(
            crate::mailbox::Options {
                relays: vec!["https://relay.invalid/".into()],
                state_dir: root.clone(),
                admission_token_file: root,
                proxy_url: None,
                ca_cert_file: None,
                poll_wait_seconds: 0,
                http1_only: false,
                http3: false,
                post_writes: false,
            },
            String::new(),
        );
        config.upstream = upstream;
        config.allow_unverified_backend = true;
        let upload = Upload {
            id: 7,
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            body: b"{}".to_vec(),
            started: Instant::now(),
            control: false,
        };
        let credits = Arc::new(Semaphore::new(RESPONSE_WINDOW));
        let (tx, mut rx) = mpsc::channel(RESPONSE_WINDOW + 4);
        let returned = credits.clone();
        let consumer = tokio::spawn(async move {
            let (mut output, mut chunks) = (Vec::new(), 0);
            while let Some(frame) = rx.recv().await {
                match frame {
                    Frame::Response { .. } => {}
                    Frame::Data { data, .. } => {
                        let bytes = decode_data(&data).unwrap();
                        assert!(!bytes.is_empty() && bytes.len() <= CHUNK_BYTES);
                        output.extend_from_slice(&bytes);
                        chunks += 1;
                        // A slow credit round trip, as across the HTTPS mailbox.
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        returned.add_permits(1);
                    }
                    Frame::End { .. } => return (output, chunks),
                    _ => panic!("unexpected frame"),
                }
            }
            panic!("stream ended without End");
        });
        let routes = Routes::new(&config).unwrap();
        forward(&routes, upload, &tx, credits).await.unwrap();
        let (output, chunks) = consumer.await.unwrap();
        assert_eq!(output, expected);
        // One credit per upstream item would need 2000+ chunks.
        assert!(chunks < 100, "{chunks} chunks");
    }

    /// A backend that names itself in every answer, with a changeable model list.
    async fn named(
        name: &'static str,
        models: Arc<std::sync::Mutex<Vec<&'static str>>>,
        private: bool,
    ) -> String {
        use axum::{
            Json,
            routing::{get, post},
        };
        let listed = models.clone();
        let router = axum::Router::new()
            .route(
                "/health",
                get(move || async move {
                    Json(if private {
                        serde_json::json!({"link_privacy":{"version":1,"application_logs":false,"request_history":false,"diagnostics":false,"remote_images":false}})
                    } else {
                        serde_json::json!({})
                    })
                }),
            )
            .route(
                "/v1/models",
                get(move || {
                    let ids = listed.lock().unwrap().clone();
                    async move {
                        Json(serde_json::json!({"object":"list","data":ids.iter()
                            .map(|id| serde_json::json!({"id":id,"object":"model","owned_by":name}))
                            .collect::<Vec<_>>()}))
                    }
                }),
            )
            .route(
                "/v1/chat/completions",
                post(move || async move { Json(serde_json::json!({"backend": name})) }),
            )
            .route(
                "/v1/embeddings",
                post(move || async move { Json(serde_json::json!({"backend": name})) }),
            )
            .route(
                "/v1/messages",
                post(move |headers: axum::http::HeaderMap| async move {
                    let header = |key: &str| {
                        headers
                            .get(key)
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("")
                            .to_owned()
                    };
                    Json(serde_json::json!({"backend": name,
                        "version": header("anthropic-version"),
                        "beta": header("anthropic-beta"),
                        "key": header("x-api-key")}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        upstream
    }
    fn routed_config(primary: String, backends: Vec<crate::config::Backend>) -> Config {
        let root = std::env::temp_dir();
        let mut config = crate::mailbox::setup::config(
            crate::mailbox::Options {
                relays: vec!["https://relay.invalid/".into()],
                state_dir: root.clone(),
                admission_token_file: root,
                proxy_url: None,
                ca_cert_file: None,
                poll_wait_seconds: 0,
                http1_only: false,
                http3: false,
                post_writes: false,
            },
            String::new(),
        );
        config.upstream = primary;
        config.backends = backends;
        config
    }
    fn backend(upstream: String, unverified: bool) -> crate::config::Backend {
        crate::config::Backend {
            upstream,
            upstream_api_key: String::new(),
            allow_unverified_backend: unverified,
            allow_network_upstream: false,
        }
    }
    /// Runs one request through `forward` and returns the response body.
    async fn ask(routes: &Routes, method: &str, path: &str, body: &str) -> Result<String> {
        let upload = Upload {
            id: 1,
            method: method.into(),
            path: path.into(),
            body: body.as_bytes().to_vec(),
            started: Instant::now(),
            control: false,
        };
        let credits = Arc::new(Semaphore::new(RESPONSE_WINDOW));
        let (tx, mut rx) = mpsc::channel(64);
        forward(routes, upload, &tx, credits).await?;
        drop(tx);
        let mut output = Vec::new();
        while let Some(frame) = rx.recv().await {
            if let Frame::Data { data, .. } = frame {
                output.extend_from_slice(&decode_data(&data).unwrap());
            }
        }
        Ok(String::from_utf8(output)?)
    }

    #[tokio::test]
    async fn requests_reach_the_backend_that_lists_their_model() {
        let primary = named("a", Arc::new(vec!["alpha", "shared"].into()), true).await;
        let second = named("b", Arc::new(vec!["beta", "shared"].into()), false).await;
        let routes = Routes::new(&routed_config(primary, vec![backend(second, true)])).unwrap();
        let chat = |model: &str| format!(r#"{{"model":"{model}","messages":[]}}"#);
        for (model, expected) in [
            ("alpha", "a"),
            ("beta", "b"),
            ("shared", "a"),
            ("unlisted", "a"),
        ] {
            let answer = ask(&routes, "POST", "/v1/chat/completions", &chat(model))
                .await
                .unwrap();
            assert!(
                answer.contains(&format!(r#""backend":"{expected}""#)),
                "{model}: {answer}"
            );
        }
        let list: serde_json::Value =
            serde_json::from_str(&ask(&routes, "GET", "/v1/models", "").await.unwrap()).unwrap();
        let ids: Vec<_> = list["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["id"].as_str().unwrap(), m["owned_by"].as_str().unwrap()))
            .collect();
        assert_eq!(ids, [("alpha", "a"), ("shared", "a"), ("beta", "b")]);
    }

    #[tokio::test]
    async fn a_listed_model_never_falls_back_when_its_backend_fails_the_privacy_check() {
        let primary = named("a", Arc::new(vec!["alpha"].into()), true).await;
        let second = named("b", Arc::new(vec!["beta"].into()), false).await;
        let routes = Routes::new(&routed_config(primary, vec![backend(second, false)])).unwrap();
        assert!(
            ask(
                &routes,
                "POST",
                "/v1/chat/completions",
                r#"{"model":"beta"}"#
            )
            .await
            .is_err()
        );
        let list = ask(&routes, "GET", "/v1/models", "").await.unwrap();
        assert!(list.contains("alpha") && !list.contains("beta"));
    }

    #[tokio::test]
    async fn without_further_backends_the_model_list_passes_through_unchanged() {
        let primary = named("a", Arc::new(vec!["alpha"].into()), true).await;
        let routes = Routes::new(&routed_config(primary, vec![])).unwrap();
        let list = ask(&routes, "GET", "/v1/models", "").await.unwrap();
        assert!(list.contains(r#""owned_by":"a""#));
        assert!(!routes.is_routed());
    }

    #[tokio::test]
    async fn a_model_switch_refreshes_the_catalog() {
        let models = Arc::new(std::sync::Mutex::new(vec!["alpha"]));
        let primary = named("a", models.clone(), true).await;
        let second = named("b", Arc::new(vec!["beta"].into()), true).await;
        let routes = Routes::new(&routed_config(primary, vec![backend(second, false)])).unwrap();
        assert!(
            !ask(&routes, "GET", "/v1/models", "")
                .await
                .unwrap()
                .contains("gamma")
        );
        models.lock().unwrap().push("gamma");
        assert!(
            !ask(&routes, "GET", "/v1/models", "")
                .await
                .unwrap()
                .contains("gamma")
        );
        routes.invalidate();
        assert!(
            ask(&routes, "GET", "/v1/models", "")
                .await
                .unwrap()
                .contains("gamma")
        );
    }

    #[tokio::test]
    async fn anthropic_and_embedding_requests_are_routed_with_a_pinned_api_version() {
        let primary = named("a", Arc::new(vec!["alpha"].into()), true).await;
        let second = named("b", Arc::new(vec!["beta"].into()), true).await;
        let mut further = backend(second, false);
        further.upstream_api_key = "backend-secret".into();
        let routes = Routes::new(&routed_config(primary, vec![further])).unwrap();
        let answer: serde_json::Value = serde_json::from_str(
            &ask(
                &routes,
                "POST",
                "/v1/messages",
                r#"{"model":"beta","max_tokens":1}"#,
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(answer["backend"], "b");
        assert_eq!(answer["version"], ANTHROPIC_VERSION);
        assert_eq!(answer["key"], "backend-secret");
        assert_eq!(answer["beta"], "");
        let primary = ask(&routes, "POST", "/v1/messages", r#"{"model":"alpha"}"#)
            .await
            .unwrap();
        assert!(primary.contains(r#""backend":"a""#) && primary.contains(r#""key":"""#));
        let embedding = ask(
            &routes,
            "POST",
            "/v1/embeddings",
            r#"{"model":"beta","input":"x"}"#,
        )
        .await
        .unwrap();
        assert!(embedding.contains(r#""backend":"b""#));
    }

    #[tokio::test]
    async fn finished_responses_kept_for_resume_are_bounded_oldest_first() {
        let mut registry = Registry::default();
        for id in 1..=LINGERING as u64 + 4 {
            let (control, _) = mpsc::unbounded_channel();
            registry.outlets.insert(
                id,
                Outlet {
                    serial: id,
                    session: None,
                    control,
                    finished: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                },
            );
        }
        let (control, _) = mpsc::unbounded_channel();
        registry.outlets.insert(
            99,
            Outlet {
                serial: 0,
                session: None,
                control,
                finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        );
        registry.trim_lingering();
        // Still-running responses stay; of the finished ones only the newest remain.
        assert!(registry.outlets.contains_key(&99));
        assert_eq!(registry.outlets.len(), LINGERING + 1);
        assert!((1..=4).all(|id| !registry.outlets.contains_key(&id)));
    }
}
