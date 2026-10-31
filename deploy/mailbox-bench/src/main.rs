//! Isolated carrier benchmark, NOT the production mailbox transport or pairing protocol.
mod carriers;
mod retention;
use anyhow::{Result, bail, ensure};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use bytes::BytesMut;
use futures_util::{StreamExt, stream::FuturesUnordered};
use k_lani_link_protocol::{Chain, Frame, Initiator, MAX_PACKET_BYTES, SecureSession, Verdict};
use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, mpsc};

const AUTH: &str = "Bearer synthetic-benchmark-only-no-production-access";
const CAP: usize = 32;
const MAX_BUFFERED: usize = 16 * 1024 * 1024;
const EVENT: &str = "data: {\"id\":\"chatcmpl-synthetic\",\"object\":\"chat.completion.chunk\",\"model\":\"no-llm\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"xxxxxxxx\"},\"finish_reason\":null}]}\n\n";

struct Queue {
    records: Mutex<BTreeMap<u64, (Instant, Bytes)>>,
    notify: Notify,
    acknowledged: AtomicU64,
    floor: AtomicU64,
    offered: AtomicU64,
    tombstones: Mutex<BTreeMap<u64, [u8; 32]>>,
}
#[derive(Default)]
struct App {
    rooms: Mutex<HashMap<String, Arc<Queue>>>,
    puts: AtomicU64,
    acks: AtomicU64,
    gets: AtomicU64,
    taken: AtomicU64,
    rejected: AtomicU64,
    buffered: AtomicUsize,
}
impl App {
    fn queue(&self, name: String) -> Option<Arc<Queue>> {
        if name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
        let mut rooms = self.rooms.lock().unwrap();
        if rooms.len() >= 128 && !rooms.contains_key(&name) {
            return None;
        }
        Some(
            rooms
                .entry(name)
                .or_insert_with(|| {
                    Arc::new(Queue {
                        records: Mutex::new(BTreeMap::new()),
                        notify: Notify::new(),
                        acknowledged: AtomicU64::new(1),
                        floor: AtomicU64::new(1),
                        offered: AtomicU64::new(0),
                        tombstones: Mutex::new(BTreeMap::new()),
                    })
                })
                .clone(),
        )
    }
}
fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(AUTH)
}
async fn box_put(
    State(app): State<Arc<App>>,
    Path((room, seq)): Path<(String, u64)>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    app.puts.fetch_add(1, Relaxed);
    if !(28..=MAX_PACKET_BYTES).contains(&body.len()) {
        return StatusCode::BAD_REQUEST;
    }
    let Some(queue) = app.queue(room) else {
        return StatusCode::TOO_MANY_REQUESTS;
    };
    retention::insert(&app, &queue, seq, body)
}

fn take_packet(buffer: &mut BytesMut) -> Result<Option<Bytes>> {
    if buffer.len() < 4 {
        return Ok(None);
    }
    let size = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
    ensure!((28..=MAX_PACKET_BYTES).contains(&size), "invalid length");
    if buffer.len() < size + 4 {
        return Ok(None);
    }
    let _ = buffer.split_to(4);
    Ok(Some(buffer.split_to(size).freeze()))
}
fn prefixed(packet: &[u8]) -> Bytes {
    let mut bytes = (packet.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(packet);
    Bytes::from(bytes)
}
async fn stream_put(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> StatusCode {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    app.puts.fetch_add(1, Relaxed);
    let Some(queue) = app.queue(room) else {
        return StatusCode::TOO_MANY_REQUESTS;
    };
    let mut chunks = body.into_data_stream();
    let mut buffer = BytesMut::new();
    while let Some(chunk) = chunks.next().await {
        let Ok(chunk) = chunk else {
            return StatusCode::BAD_REQUEST;
        };
        buffer.extend_from_slice(&chunk);
        loop {
            match take_packet(&mut buffer) {
                Ok(Some(packet)) => {
                    let Ok(seq) = retention::index(&packet) else {
                        return StatusCode::BAD_REQUEST;
                    };
                    let status = retention::insert_wait(&app, &queue, seq, packet).await;
                    if status != StatusCode::NO_CONTENT {
                        return status;
                    }
                }
                Ok(None) => break,
                Err(_) => return StatusCode::BAD_REQUEST,
            }
        }
    }
    if buffer.is_empty() {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::BAD_REQUEST
    }
}
fn number(path: &str) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}
fn cpu_usage() -> u64 {
    let cpu = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").unwrap_or_default();
    cpu.lines()
        .find_map(|line| {
            line.strip_prefix("usage_usec ")
                .and_then(|n| n.parse().ok())
        })
        .unwrap_or(0)
}
async fn stats(State(app): State<Arc<App>>) -> Json<serde_json::Value> {
    let usage = cpu_usage();
    Json(
        serde_json::json!({"puts":app.puts.load(Relaxed),"acks":app.acks.load(Relaxed),"gets":app.gets.load(Relaxed),"records_taken":app.taken.load(Relaxed),"rejected":app.rejected.load(Relaxed),
        "buffered_bytes":app.buffered.load(Relaxed),"cpu_usage_us":usage,
        "memory_current":number("/sys/fs/cgroup/memory.current"),"memory_peak":number("/sys/fs/cgroup/memory.peak"),
        "rx_bytes":number("/sys/class/net/eth0/statistics/rx_bytes"),"tx_bytes":number("/sys/class/net/eth0/statistics/tx_bytes")}),
    )
}
async fn serve() -> Result<()> {
    let app = Arc::new(App::default());
    let sweeper = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let queues: Vec<_> = sweeper.rooms.lock().unwrap().values().cloned().collect();
            for queue in queues {
                retention::expire(&sweeper, &queue);
            }
        }
    });
    let router = Router::new()
        .route("/box/{room}/{seq}", put(box_put))
        .route("/stream/{room}", put(stream_put).get(carriers::stream_get))
        .route("/post/{room}/{seq}", post(box_put))
        .route("/read/{room}/{kind}", get(carriers::read))
        .route("/ack/{room}", post(retention::ack))
        .route("/ws/{room}/{direction}", get(carriers::websocket))
        .route("/stats", get(stats))
        .layer(DefaultBodyLimit::max(MAX_PACKET_BYTES))
        .with_state(app);
    if std::env::var("BENCH_TLS").as_deref() == Ok("direct") {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(
            "/cert/server.crt",
            "/cert/server.key",
        )
        .await?;
        axum_server::bind_rustls("0.0.0.0:8443".parse::<std::net::SocketAddr>()?, config)
            .map(|acceptor| acceptor.acceptor(axum_server::accept::NoDelayAcceptor::new()))
            .serve(router.into_make_service())
            .await?;
    } else {
        axum::serve(tokio::net::TcpListener::bind("0.0.0.0:8080").await?, router).await?;
    }
    Ok(())
}
#[derive(Debug)]
struct Terminal(&'static str);
impl std::fmt::Display for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Terminal {}
struct Observation {
    reconnects: AtomicU64,
    injected_cut: std::sync::atomic::AtomicBool,
    resuming: Mutex<Option<Instant>>,
    resume_ms: Mutex<Vec<f64>>,
    first_token_ms: Mutex<Option<f64>>,
    timestamps: Mutex<HashMap<u64, (Instant, usize)>>,
    latencies_ms: Mutex<Vec<f64>>,
    tokens: AtomicU64,
    records: AtomicU64,
    bytes: AtomicU64,
}
impl Observation {
    fn new() -> Self {
        Self {
            first_token_ms: Mutex::new(None),
            reconnects: AtomicU64::new(0),
            injected_cut: std::sync::atomic::AtomicBool::new(false),
            resuming: Mutex::new(None),
            resume_ms: Mutex::new(Vec::new()),
            timestamps: Mutex::new(HashMap::new()),
            latencies_ms: Mutex::new(vec![]),
            tokens: AtomicU64::new(0),
            records: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }
    fn receive(&self, session: &mut SecureSession, packet: &[u8], rate: usize) -> Result<bool> {
        let index = session.receive_sequence() + 1;
        let frames = session.open_batch(packet).map_err(anyhow::Error::msg)?;
        if matches!(frames.as_slice(), [Frame::End { id: 1 }]) {
            return Ok(true);
        }
        let (first, count) = self
            .timestamps
            .lock()
            .unwrap()
            .remove(&index)
            .ok_or_else(|| anyhow::anyhow!("unexpected index"))?;
        let mut raw = 0;
        for frame in frames {
            let Frame::Data { data, .. } = frame else {
                bail!("unexpected frame");
            };
            raw += k_lani_link_protocol::decode_data(&data)
                .map_err(anyhow::Error::msg)?
                .len();
        }
        ensure!(raw == EVENT.len() * count, "payload mismatch");
        self.bytes.fetch_add(raw as u64, Relaxed);
        self.tokens.fetch_add(count as u64, Relaxed);
        let now = Instant::now();
        if let Some(since) = self.resuming.lock().unwrap().take() {
            self.resume_ms
                .lock()
                .unwrap()
                .push(since.elapsed().as_secs_f64() * 1000.0);
        }
        if self.records.fetch_add(1, Relaxed) == 0 {
            *self.first_token_ms.lock().unwrap() =
                Some(now.saturating_duration_since(first).as_secs_f64() * 1000.0);
        }
        let mut latencies = self.latencies_ms.lock().unwrap();
        for index in 0..count {
            latencies.push(
                now.saturating_duration_since(
                    first + Duration::from_secs_f64(index as f64 / rate as f64),
                )
                .as_secs_f64()
                    * 1000.0,
            );
        }
        Ok(false)
    }
}
fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).unwrap();
    bytes
}
fn sessions() -> Result<(String, SecureSession, SecureSession)> {
    let chain = Chain::from_bytes(random());
    let init = Initiator::start(&chain, random(), random(), random(), 1000)?;
    let room = init.room();
    let Verdict::Accepted(host) = k_lani_link_protocol::HostChains::new(chain).accept(
        &room,
        init.hello(),
        random(),
        random(),
        1000,
    )?
    else {
        bail!("setup rejected");
    };
    let client = init.finish(host.reply(), 1000)?;
    // SSE output flows host -> client. The handshake is deliberately outside the measured carrier.
    Ok((room, host.session, client.session))
}
async fn put_record(http: reqwest::Client, url: String, packet: Bytes, post: bool) -> Result<()> {
    for attempt in 0..3 {
        if let Ok(response) = http
            .request(
                if post {
                    reqwest::Method::POST
                } else {
                    reqwest::Method::PUT
                },
                &url,
            )
            .header("authorization", AUTH)
            .body(packet.clone())
            .send()
            .await
            && response.status().is_success()
        {
            return Ok(());
        }
        if attempt != 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    bail!("put failed")
}
struct Reader {
    http: reqwest::Client,
    base: String,
    room: String,
    mode: String,
    session: SecureSession,
    obs: Arc<Observation>,
    rate: usize,
    cut_after: u64,
}
impl Reader {
    async fn acknowledge(&self) -> Result<()> {
        let from = self.session.receive_sequence() + 1;
        let response = self
            .http
            .post(format!("{}/ack/{}?from={from}", self.base, self.room))
            .header("authorization", AUTH)
            .send()
            .await?;
        if response.status() == StatusCode::GONE {
            return Err(Terminal("expired_or_acknowledged_gap").into());
        }
        response.error_for_status()?;
        Ok(())
    }
    async fn packet(&mut self, packet: &[u8]) -> Result<Option<bool>> {
        if self.cut_after != 0
            && self.obs.records.load(Relaxed) >= self.cut_after
            && !self.obs.injected_cut.swap(true, Relaxed)
        {
            return Ok(None); // Discard this unverified record, close the transport, resume at the same cursor.
        }
        let end = self
            .obs
            .receive(&mut self.session, packet, self.rate)
            .map_err(|_| Terminal("integrity_or_order_error"))?;
        if end
            || (!matches!(self.mode.as_str(), "mailbox" | "longpoll")
                && self.session.receive_sequence().is_multiple_of(8))
        {
            self.acknowledge().await?;
        }
        Ok(Some(end))
    }
    async fn connection(&mut self) -> Result<bool> {
        let from = self.session.receive_sequence() + 1;
        if self.mode == "wss" {
            let mut socket = carriers::connect(&self.base, &self.room, "receive", from).await?;
            while let Some(message) = socket.next().await {
                if let tokio_tungstenite::tungstenite::Message::Binary(packet) = message? {
                    match self.packet(&packet).await? {
                        Some(false) => {}
                        Some(true) => return Ok(true),
                        None => return Ok(false),
                    }
                }
            }
            bail!("websocket ended");
        }
        let path = match self.mode.as_str() {
            "mailbox" => format!("read/{}/mailbox", self.room),
            "longpoll" => format!("read/{}/poll", self.room),
            "post-stream" => format!("read/{}/stream", self.room),
            "sse" => format!("read/{}/sse", self.room),
            _ => format!("stream/{}", self.room),
        };
        let response = self
            .http
            .get(format!("{}/{path}?from={from}", self.base))
            .header("authorization", AUTH)
            .send()
            .await?;
        if response.status() == StatusCode::GONE {
            return Err(Terminal("expired_or_acknowledged_gap").into());
        }
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(false);
        }
        let response = response.error_for_status()?;
        if self.mode == "h2-stream" {
            ensure!(
                response.version() == reqwest::Version::HTTP_2,
                "HTTP/2 not negotiated"
            );
        }
        let mut chunks = response.bytes_stream();
        let mut buffer = BytesMut::new();
        while let Some(chunk) = chunks.next().await {
            buffer.extend_from_slice(&chunk?);
            while let Some(packet) = if self.mode == "sse" {
                carriers::take_sse(&mut buffer)
            } else {
                take_packet(&mut buffer)
            }
            .map_err(|_| Terminal("invalid_record_envelope"))?
            {
                match self.packet(&packet).await? {
                    Some(false) => {}
                    Some(true) => return Ok(true),
                    None => return Ok(false),
                }
            }
        }
        ensure!(buffer.is_empty(), "incomplete body");
        if matches!(self.mode.as_str(), "mailbox" | "longpoll") {
            Ok(false)
        } else {
            bail!("stream ended")
        }
    }
}
async fn receiver(
    http: reqwest::Client,
    base: String,
    room: String,
    mode: String,
    session: SecureSession,
    obs: Arc<Observation>,
    rate: usize,
) -> Result<()> {
    let cut_after = std::env::var("BENCH_CUT_AFTER_RECORDS")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    let mut reader = Reader {
        http,
        base,
        room,
        mode,
        session,
        obs,
        rate,
        cut_after,
    };
    loop {
        let before = reader.session.receive_sequence();
        let was_cut = reader.obs.injected_cut.load(Relaxed);
        let outcome = reader.connection().await;
        match outcome {
            Ok(true) => return Ok(()),
            Err(error) if error.is::<Terminal>() => return Err(error),
            Ok(false)
                if matches!(reader.mode.as_str(), "mailbox" | "longpoll")
                    && was_cut == reader.obs.injected_cut.load(Relaxed) =>
            {
                continue;
            }
            _ => {}
        }
        // No re-pairing and no replay of backend requests: only the still-live encrypted reader resumes.
        reader.obs.reconnects.fetch_add(1, Relaxed);
        if reader.obs.resuming.lock().unwrap().is_none() {
            *reader.obs.resuming.lock().unwrap() = Some(Instant::now());
        }
        tokio::time::sleep(Duration::from_millis(
            if reader.session.receive_sequence() == before {
                100
            } else {
                20
            },
        ))
        .await;
    }
}
async fn sender(
    http: reqwest::Client,
    base: String,
    room: String,
    mode: String,
    mut session: SecureSession,
    obs: Arc<Observation>,
    workload: (usize, usize),
) -> Result<()> {
    let (rate, seconds) = workload;
    let idle: u64 = std::env::var("BENCH_IDLE_SECONDS")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    let (tx, mut rx) = mpsc::channel::<Bytes>(8);
    let stream_sender = async {
        if mode == "wss" {
            return carriers::ws_send(&base, &room, &mut rx).await;
        }
        let body = async_stream::stream! {
            while let Some(packet) = rx.recv().await { yield Ok::<Bytes,Infallible>(prefixed(&packet)); }
        };
        if mode == "stream" || mode == "h2-stream" {
            http.put(format!("{base}/stream/{room}"))
                .header("authorization", AUTH)
                .body(reqwest::Body::wrap_stream(body))
                .send()
                .await?
                .error_for_status()?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let produce = async {
        let start = Instant::now();
        let total = rate * seconds;
        let timed = (rate * 30).div_ceil(1000).max(1);
        let size_limit = k_lani_link_protocol::CHUNK_BYTES / EVENT.len();
        let group = timed.min(size_limit).max(1);
        let mut produced = 0;
        let mut pending = tokio::task::JoinSet::new();
        while produced < total {
            let count = group.min(total - produced);
            let pause_at = (total / 2 / group) * group;
            let pause = if idle != 0 && produced >= pause_at {
                Duration::from_secs(idle)
            } else {
                Duration::ZERO
            };
            let first = start + Duration::from_secs_f64(produced as f64 / rate as f64) + pause;
            let batch_delay = if size_limit < timed {
                Duration::from_secs_f64((count - 1) as f64 / rate as f64)
            } else {
                Duration::from_millis(30)
            };
            tokio::time::sleep_until((first + batch_delay).into()).await;
            let raw = EVENT.repeat(count);
            let packet = Bytes::from(
                session
                    .seal_batch(&[Frame::data(1, raw.as_bytes())])
                    .map_err(anyhow::Error::msg)?,
            );
            let index = session.send_sequence();
            obs.timestamps.lock().unwrap().insert(index, (first, count));
            if carriers::finite(&mode) {
                if pending.len() >= 8 {
                    pending.join_next().await.unwrap()??;
                }
                pending.spawn(put_record(
                    http.clone(),
                    format!(
                        "{base}/{}/{room}/{index}",
                        if mode == "mailbox" { "box" } else { "post" }
                    ),
                    packet,
                    mode != "mailbox",
                ));
            } else {
                tx.send(packet).await?;
            }
            produced += count;
        }
        while let Some(result) = pending.join_next().await {
            result??;
        }
        let end = Bytes::from(
            session
                .seal_batch(&[Frame::End { id: 1 }])
                .map_err(anyhow::Error::msg)?,
        );
        if carriers::finite(&mode) {
            put_record(
                http.clone(),
                format!(
                    "{base}/{}/{room}/{}",
                    if mode == "mailbox" { "box" } else { "post" },
                    session.send_sequence()
                ),
                end,
                mode != "mailbox",
            )
            .await?;
        } else {
            tx.send(end).await?;
        }
        drop(tx);
        Ok::<_, anyhow::Error>(())
    };
    tokio::try_join!(produce, stream_sender)?;
    Ok(())
}
async fn run(args: &[String]) -> Result<()> {
    let mode = args
        .get(2)
        .ok_or_else(|| anyhow::anyhow!("missing mode"))?
        .clone();
    ensure!(carriers::MODES.contains(&mode.as_str()), "invalid mode");
    let rate: usize = args
        .get(3)
        .ok_or_else(|| anyhow::anyhow!("missing rate"))?
        .parse()?;
    let seconds: usize = args
        .get(4)
        .ok_or_else(|| anyhow::anyhow!("missing duration"))?
        .parse()?;
    let tunnels: usize = args
        .get(5)
        .ok_or_else(|| anyhow::anyhow!("missing count"))?
        .parse()?;
    ensure!(
        (1..=1000).contains(&rate) && (1..=60).contains(&seconds) && (1..=100).contains(&tunnels),
        "invalid workload"
    );
    let base = std::env::var("BENCH_BASE")?;
    let ca = reqwest::Certificate::from_pem(&std::fs::read(std::env::var("BENCH_CA")?)?)?;
    let idle: u64 = std::env::var("BENCH_IDLE_SECONDS")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    ensure!(idle <= 120, "invalid idle interval");
    let client = || -> Result<reqwest::Client> {
        let builder = reqwest::Client::builder().no_proxy();
        let builder = if mode == "h2-stream" {
            builder.http2_prior_knowledge()
        } else {
            builder.http1_only()
        };
        Ok(builder
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(ca.clone())
            .timeout(Duration::from_secs(seconds as u64 + idle + 35))
            .pool_max_idle_per_host(16)
            .build()?)
    };
    let http = client()?;
    let mut rtts = Vec::new();
    for _ in 0..5 {
        let ping = Instant::now();
        let response = http
            .get(format!("{base}/stats"))
            .send()
            .await?
            .error_for_status()?;
        if mode == "h2-stream" {
            ensure!(
                response.version() == reqwest::Version::HTTP_2,
                "HTTP/2 not negotiated"
            );
        }
        response.bytes().await?;
        rtts.push(ping.elapsed().as_secs_f64() * 1000.0);
    }
    rtts.sort_by(f64::total_cmp);
    let before: serde_json::Value = http
        .get(format!("{base}/stats"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let client_cpu_before = cpu_usage();
    let drain: u64 = std::env::var("BENCH_DRAIN_SECONDS")
        .unwrap_or_else(|_| "10".into())
        .parse()?;
    ensure!((1..=60).contains(&drain), "invalid drain timeout");
    let start = Instant::now();
    let mut jobs = FuturesUnordered::new();
    let mut observations = Vec::new();
    for _ in 0..tunnels {
        let (room, send, receive) = sessions()?;
        let obs = Arc::new(Observation::new());
        observations.push(obs.clone());
        let (upload, download, base, mode) = (client()?, client()?, base.clone(), mode.clone());
        jobs.push(tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(seconds as u64 + idle + drain), async {
                tokio::try_join!(
                    sender(
                        upload,
                        base.clone(),
                        room.clone(),
                        mode.clone(),
                        send,
                        obs.clone(),
                        (rate, seconds)
                    ),
                    receiver(download, base, room, mode, receive, obs, rate)
                )
            })
            .await
            {
                Ok(Ok(_)) => "complete",
                Ok(Err(error)) => error
                    .downcast_ref::<Terminal>()
                    .map_or("transport_error", |error| error.0),
                Err(_) => "deadline",
            }
        }));
    }
    let mut failures = 0;
    let mut failure_kinds = BTreeMap::new();
    while let Some(result) = jobs.next().await {
        let outcome = result.unwrap_or("task_failed");
        if outcome != "complete" {
            failures += 1;
            *failure_kinds.entry(outcome).or_insert(0usize) += 1;
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let client_cpu_cores =
        cpu_usage().saturating_sub(client_cpu_before) as f64 / 1_000_000.0 / elapsed;
    let after: serde_json::Value = http
        .get(format!("{base}/stats"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut latencies: Vec<f64> = observations
        .iter()
        .flat_map(|o| o.latencies_ms.lock().unwrap().clone())
        .collect();
    latencies.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        if latencies.is_empty() {
            None
        } else {
            Some(latencies[((latencies.len() - 1) as f64 * p).round() as usize])
        }
    };
    let tokens: u64 = observations.iter().map(|o| o.tokens.load(Relaxed)).sum();
    let records: u64 = observations.iter().map(|o| o.records.load(Relaxed)).sum();
    let first: Vec<f64> = observations
        .iter()
        .filter_map(|o| *o.first_token_ms.lock().unwrap())
        .collect();
    let first_max = first.into_iter().max_by(f64::total_cmp);
    let reconnects: u64 = observations
        .iter()
        .map(|o| o.reconnects.load(Relaxed))
        .sum();
    let injected_cuts = observations
        .iter()
        .filter(|o| o.injected_cut.load(Relaxed))
        .count();
    let recovery_max = observations
        .iter()
        .flat_map(|o| o.resume_ms.lock().unwrap().clone())
        .max_by(f64::total_cmp);
    let delta = |key: &str| {
        after[key]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(before[key].as_u64().unwrap_or(0))
    };
    println!(
        "{}",
        serde_json::json!({"mode":mode,"http":if mode == "h2-stream" { "2 with TLS" } else if mode == "wss" { "WebSocket over TLS (HTTP/1.1 upgrade)" } else { "1.1 with TLS" },"synthetic_tokens_per_second_per_tunnel":rate,"offered_tokens":rate*seconds*tunnels,
        "tunnels":tunnels,"event_bytes":EVENT.len(),"generation_seconds":seconds,"elapsed_seconds":elapsed,"received_tokens":tokens,"records":records,"https_rtt_median_ms":rtts[2],
        "delivered_tokens_per_second":tokens as f64/elapsed,"failed_tunnels":failures,"failure_kinds":failure_kinds,"first_token_max_ms":first_max,"latency_p50_ms":percentile(0.50),"latency_p99_ms":percentile(0.99),
        "reader_reconnects":reconnects,"injected_cuts":injected_cuts,"recovery_until_data_max_ms":recovery_max,"idle_seconds":idle,"ack_requests":delta("acks"),"put_requests":delta("puts"),"get_requests":delta("gets"),"http_requests_per_second":(delta("puts")+delta("gets")+delta("acks")) as f64/elapsed,
        "rejected_puts":delta("rejected"),"relay_cpu_cores":delta("cpu_usage_us") as f64/1_000_000.0/elapsed,
        "client_cpu_cores":client_cpu_cores,"client_memory_peak":number("/sys/fs/cgroup/memory.peak"),"drain_seconds":drain,"relay_memory_peak":after["memory_peak"],"relay_rx_bytes":delta("rx_bytes"),"relay_tx_bytes":delta("tx_bytes")})
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<_> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("serve") => serve().await,
        Some("run") => run(&args).await,
        _ => Err(anyhow::anyhow!("invalid command")),
    };
    if result.is_err() {
        eprintln!("benchmark setup or transport failed");
        std::process::exit(1);
    }
}
