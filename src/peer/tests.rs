use super::*;
use crate::{config, gateway::Gateway, host, pairing::PairingLease};
use axum::{
    Json, Router,
    body::Body,
    response::Response,
    routing::{get, post},
};
use futures_util::StreamExt;
use std::{
    convert::Infallible,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::atomic::AtomicUsize,
    task::{Context, Poll},
};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime,
    TokioRuntime, Transmit,
};

/// Drop datagrams at the real socket boundary, without changing the machine's network.
#[derive(Debug)]
struct LossRuntime(Arc<AtomicBool>);
#[derive(Debug)]
struct LossSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    blocked: Arc<AtomicBool>,
}
impl AsyncUdpSocket for LossSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn poll_send(&self, cx: &mut Context<'_>, transmit: &Transmit<'_>) -> Poll<io::Result<usize>> {
        if self.blocked.load(Ordering::SeqCst) {
            Poll::Ready(Ok(transmit.contents.len()))
        } else {
            self.inner.poll_send(cx, transmit)
        }
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_recv(cx, bufs, meta)
    }
    fn max_gso_segments(&self) -> usize {
        self.inner.max_gso_segments()
    }
    fn max_gro_segments(&self) -> usize {
        self.inner.max_gro_segments()
    }
}
impl Runtime for LossRuntime {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        TokioRuntime.spawn(future)
    }
    fn wrap_udp_socket(&self, socket: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        Ok(Arc::new(LossSocket {
            inner: TokioRuntime.wrap_udp_socket(socket)?,
            blocked: self.0.clone(),
        }))
    }
    fn wrap_tcp_listener(
        &self,
        listener: std::net::TcpListener,
    ) -> io::Result<Arc<dyn AsyncTcpListener>> {
        TokioRuntime.wrap_tcp_listener(listener)
    }
    fn connect_tcp<'a>(
        &'a self,
        remote: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        TokioRuntime.connect_tcp(remote)
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        TokioRuntime.resolve_host(host)
    }
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        TokioRuntime.sleep(duration)
    }
    fn interval(&self, period: Duration) -> Box<dyn AsyncInterval> {
        TokioRuntime.interval(period)
    }
    fn block_on(&self, future: Pin<Box<dyn Future<Output = ()> + '_>>) {
        TokioRuntime.block_on(future)
    }
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, task)
}

struct Finished(Arc<AtomicUsize>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

async fn wait_until(timeout: u64, condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(timeout), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("expected state transition did not occur");
}

async fn connected_pair(cfg: &Config) -> (Peer, Peer) {
    let client = Peer::new(cfg, true).await.unwrap();
    let home = Peer::new(cfg, false).await.unwrap();
    let offer = seal(
        cfg,
        &config::random_id().unwrap(),
        "offer",
        &client.local_sdp(true).await.unwrap(),
        EndpointRole::Client,
    )
    .unwrap();
    home.set_remote(
        crate::pairing::open(&offer, cfg, "offer", EndpointRole::Client).unwrap(),
        true,
        cfg.relay,
    )
    .await
    .unwrap();
    let answer = seal(
        cfg,
        &offer.context.session,
        "answer",
        &home.local_sdp(false).await.unwrap(),
        EndpointRole::Host,
    )
    .unwrap();
    for (peer, role) in [(&home, EndpointRole::Host), (&client, EndpointRole::Client)] {
        peer.install_secure_session(&cfg.pair_key, &offer, &answer, role, EndpointRole::Client)
            .unwrap();
    }
    client
        .set_remote(
            crate::pairing::open(&answer, cfg, "answer", EndpointRole::Host).unwrap(),
            false,
            cfg.relay,
        )
        .await
        .unwrap();
    tokio::try_join!(client.wait_open(), home.wait_open()).unwrap();
    (client, home)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_close_and_drop_stop_a_peer_with_full_receive_queue() {
    let root = std::env::temp_dir().join(format!("link-close-{}", config::random_id().unwrap()));
    config::generate_pair(&root, "http://127.0.0.1:9", Default::default(), vec![]).unwrap();
    let cfg = Config::load(&root.join("host.toml")).unwrap();
    for explicit_close in [true, false] {
        let (client, home) = connected_pair(&cfg).await;
        let tx = client.tx.clone();
        let sending = tokio::spawn(async move {
            let bytes = [0_u8; 1024];
            while tx.send(Frame::data(7, &bytes)).await.is_ok() {}
        });
        // Deliberately do not consume incoming application frames: cancellation
        // must bypass the congested data path, including the blocked send().
        wait_until(5, || home.rx.len() == 32).await;
        let closed = home.closed.clone();
        if explicit_close {
            tokio::time::timeout(Duration::from_secs(2), home.close())
                .await
                .expect("local close must not wait for the application queue to drain");
        }
        drop(home);
        assert!(closed.is_cancelled());
        tokio::time::timeout(Duration::from_secs(3), client.closed.cancelled())
            .await
            .expect("remote transport must observe the closed peer");
        tokio::time::timeout(Duration::from_secs(2), sending)
            .await
            .expect("outbound application loop must stop after close")
            .unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_stream_recovers_in_same_session_but_permanent_loss_ends_it() {
    let root = std::env::temp_dir().join(format!("link-loss-{}", config::random_id().unwrap()));
    // A closed local port: the test exchanges setup messages directly. Recovery
    // must work with no reachable broker and no new signaling at all.
    config::generate_pair(&root, "http://127.0.0.1:9", Default::default(), vec![]).unwrap();
    let mut cfg = Config::load(&root.join("host.toml")).unwrap();
    cfg.request_timeout_seconds = 60;
    let requests = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let produce = CancellationToken::new();
    let calls = requests.clone();
    let done = finished.clone();
    let gate = produce.clone();
    let mock = Router::new()
        .route("/health", get(|| async { Json(serde_json::json!({"link_privacy":{
            "version":1,"application_logs":false,"request_history":false,"diagnostics":false,"remote_images":false
        }})) }))
        .route("/v1/chat/completions", post(move || {
            let number = calls.fetch_add(1, Ordering::SeqCst);
            let guard = Finished(done.clone());
            let gate = gate.clone();
            async move {
                let stream = async_stream::stream! {
                    let _guard = guard;
                    yield Ok::<_, Infallible>(bytes::Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"}}]}\n\n"));
                    if number > 0 { std::future::pending::<()>().await; }
                    gate.cancelled().await;
                    for i in 0..40 {
                        let event = serde_json::json!({"choices":[{"delta":{"content":format!("token-{i:03}")}}]});
                        yield Ok(bytes::Bytes::from(format!("data: {event}\n\n")));
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    yield Ok(bytes::Bytes::from_static(b"data: [DONE]\n\n"));
                };
                Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(stream)).unwrap()
            }
        }));
    let (upstream, mock_task) = serve(mock).await;
    cfg.upstream = upstream;
    let blocked = Arc::new(AtomicBool::new(false));
    let runtime = Arc::new(LossRuntime(blocked.clone()));
    let client = Peer::build(
        &cfg,
        true,
        PeerConnectionBuilder::new().with_runtime(runtime.clone()),
    )
    .await
    .unwrap();
    let home = Peer::build(
        &cfg,
        false,
        PeerConnectionBuilder::new().with_runtime(runtime),
    )
    .await
    .unwrap();
    let offer = seal(
        &cfg,
        &config::random_id().unwrap(),
        "offer",
        &client.local_sdp(true).await.unwrap(),
        EndpointRole::Client,
    )
    .unwrap();
    home.set_remote(
        crate::pairing::open(&offer, &cfg, "offer", EndpointRole::Client).unwrap(),
        true,
        cfg.relay,
    )
    .await
    .unwrap();
    let answer = seal(
        &cfg,
        &offer.context.session,
        "answer",
        &home.local_sdp(false).await.unwrap(),
        EndpointRole::Host,
    )
    .unwrap();
    home.install_secure_session(
        &cfg.pair_key,
        &offer,
        &answer,
        EndpointRole::Host,
        EndpointRole::Client,
    )
    .unwrap();
    client
        .install_secure_session(
            &cfg.pair_key,
            &offer,
            &answer,
            EndpointRole::Client,
            EndpointRole::Client,
        )
        .unwrap();
    client
        .set_remote(
            crate::pairing::open(&answer, &cfg, "answer", EndpointRole::Host).unwrap(),
            false,
            cfg.relay,
        )
        .await
        .unwrap();
    tokio::try_join!(client.wait_open(), home.wait_open()).unwrap();
    // The code is already spent. A short outage must preserve only this session.
    let state_dir = root.join("used");
    let mut lease = PairingLease::reserve(&state_dir, &cfg.pair_key).unwrap();
    lease.consume().unwrap();
    let recovering = client.recovering.clone();
    let closed = client.closed.clone();
    let host_cfg = cfg.clone();
    let host_task = tokio::spawn(async move {
        host::serve(&host_cfg, home).await.unwrap();
    });
    let gateway = Gateway::new(cfg.clone()).unwrap();
    let background = gateway.clone();
    let gateway_task = tokio::spawn(async move {
        background.attach(client).await;
    });
    wait_until(2, || gateway.is_connected()).await;
    let (api, api_task) = serve(gateway.router()).await;
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();
    let response = http
        .post(format!("{api}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"synthetic","stream":true}))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    assert!(
        String::from_utf8_lossy(&stream.next().await.unwrap().unwrap())
            .contains("reasoning_content")
    );
    blocked.store(true, Ordering::SeqCst);
    produce.cancel();
    wait_until(12, || recovering.load(Ordering::SeqCst)).await;
    assert!(
        !closed.is_cancelled(),
        "temporary loss must not destroy the secure session"
    );
    let health: serde_json::Value = http
        .get(format!("{api}/health"))
        .bearer_auth(&cfg.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["recovering"], true);
    assert_eq!(health["connected"], false);
    assert_eq!(health["new_code_required"], false);
    assert!(
        tokio::time::timeout(Duration::from_millis(250), stream.next())
            .await
            .is_err()
    );
    blocked.store(false, Ordering::SeqCst);
    let remaining = tokio::time::timeout(Duration::from_secs(12), async {
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            bytes.extend_from_slice(&chunk.unwrap());
        }
        String::from_utf8(bytes).unwrap()
    })
    .await
    .expect("the same SSE response must continue after packet delivery resumes");
    let mut last = 0;
    for i in 0..40 {
        let token = format!("token-{i:03}");
        assert_eq!(
            remaining.matches(&token).count(),
            1,
            "no lost or repeated token"
        );
        let position = remaining.find(&token).unwrap();
        assert!(position >= last, "token order must be preserved");
        last = position;
    }
    assert!(remaining.ends_with("data: [DONE]\n\n"));
    assert_eq!(requests.load(Ordering::SeqCst), 1, "no replayed inference");
    wait_until(3, || !recovering.load(Ordering::SeqCst)).await;
    assert!(PairingLease::reserve(&state_dir, &cfg.pair_key).is_err());

    // A second response is deliberately left open, then connectivity never returns.
    let response = http
        .post(format!("{api}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"synthetic","stream":true}))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    stream.next().await.unwrap().unwrap();
    blocked.store(true, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(28), closed.cancelled())
        .await
        .expect("permanent loss must terminate within the ICE failure bound");
    let next = tokio::time::timeout(Duration::from_secs(3), stream.next())
        .await
        .unwrap();
    assert!(
        next.is_some_and(|chunk| chunk.is_err()),
        "incomplete SSE must not report success"
    );
    wait_until(3, || finished.load(Ordering::SeqCst) == 2).await;
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    tokio::time::timeout(Duration::from_secs(3), host_task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), gateway_task)
        .await
        .unwrap()
        .unwrap();
    api_task.abort();
    mock_task.abort();
    drop(lease);
    std::fs::remove_dir_all(root).unwrap();
}
