//! Test against the wire contract in Peer2Download/tools/ice_exchange_smoke.js.
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use k_lani_link::{
    config::{Config, RelayPolicy, SignalingBackend},
    peer,
};
use k_lani_link_protocol::Frame;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Default)]
struct Exchange {
    joins: Arc<AtomicUsize>,
    offer: Arc<Mutex<Option<Value>>>,
    answer: Arc<Mutex<Option<Value>>>,
    calls: Arc<AtomicUsize>,
    answer_delay: Duration,
}
async fn wait_value(slot: &Mutex<Option<Value>>) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(value) = slot.lock().unwrap().clone() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
async fn join(State(state): State<Exchange>) -> Json<Value> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let first = state.joins.fetch_add(1, Ordering::SeqCst) == 0;
    if first {
        while state.joins.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    Json(json!({"role":if first {"offerer"} else {"answerer"}}))
}
async fn offer(State(state): State<Exchange>, Json(value): Json<Value>) -> Json<Value> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    assert!(value["sdp"].as_str().unwrap().starts_with("v=0\r\n"));
    assert_eq!(value["ice"], json!([]));
    *state.offer.lock().unwrap() = Some(value);
    Json(wait_value(&state.answer).await)
}
async fn get_offer(State(state): State<Exchange>) -> Json<Value> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    let mut value = wait_value(&state.offer).await;
    value["seq"] = json!(1);
    Json(value)
}
async fn answer(State(state): State<Exchange>, Json(value): Json<Value>) -> Json<Value> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    *state.answer.lock().unwrap() = Some(value);
    tokio::time::sleep(state.answer_delay).await;
    Json(json!({"ok":true}))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_exchange_contract_connects_and_detaches() {
    check_connection_and_detachment(Duration::ZERO).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opening_data_channel_closes_even_a_pending_signaling_acknowledgement() {
    check_connection_and_detachment(Duration::from_secs(30)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mailbox_also_drops_a_delayed_answer_acknowledgement() {
    async fn delay_answer(
        request: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> axum::response::Response {
        let delay = request.method() == axum::http::Method::PUT
            && request.uri().path().ends_with("/answer");
        let response = next.run(request).await;
        if delay {
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
        response
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app =
        k_lani_link::signal::router("s".repeat(32)).layer(axum::middleware::from_fn(delay_answer));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (url, active, proxy) = connection_probe(address).await;
    let config = Config {
        transport: Default::default(),
        https: None,
        signaling_url: url,
        signaling_backend: SignalingBackend::Mailbox,
        signaling_token: "s".repeat(32),
        room: k_lani_link_protocol::room_from_key(&URL_SAFE_NO_PAD.encode([42; 32])).unwrap(),
        pair_key: URL_SAFE_NO_PAD.encode([42; 32]),
        pairing_state_dir: None,
        api_key: "a".repeat(32),
        listen: "127.0.0.1:8787".parse().unwrap(),
        upstream: "http://127.0.0.1:8080".into(),
        upstream_socket: None,
        upstream_api_key: String::new(),
        relay: RelayPolicy::Disabled,
        ice_servers: vec![],
        request_timeout_seconds: 60,
        allow_unverified_backend: false,
        include_loopback_candidate: true,
        resume_seconds: 0,
        backends: vec![],
        max_concurrent_requests: 1,
        max_queued_requests: 0,
        management_token_file: None,
        model_control_socket: None,
    };
    let host_config = config.clone();
    let host = tokio::spawn(async move {
        peer::accept(&host_config, &mut String::new())
            .await
            .unwrap()
    });
    let client = tokio::time::timeout(Duration::from_secs(10), peer::connect(&config))
        .await
        .unwrap()
        .unwrap();
    let host = tokio::time::timeout(Duration::from_secs(3), host)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while active.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("mailbox connection must close without waiting for the acknowledgement");
    client.close().await;
    host.close().await;
    proxy.abort();
    server.abort();
}

async fn check_connection_and_detachment(answer_delay: Duration) {
    let exchange = Exchange {
        answer_delay,
        ..Default::default()
    };
    let app = Router::new()
        .route("/{room}", post(join))
        .route("/{room}/offer", get(get_offer).post(offer))
        .route("/{room}/answer", post(answer))
        .with_state(exchange.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let server_address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Observe the actual TCP lifecycle, independently of the HTTP handler's future.
    let (url, active_connections, proxy) = connection_probe(server_address).await;
    let config = Config {
        transport: Default::default(),
        https: None,
        signaling_url: url,
        signaling_backend: SignalingBackend::IceExchange,
        signaling_token: String::new(),
        room: k_lani_link_protocol::room_from_key(&URL_SAFE_NO_PAD.encode([42; 32])).unwrap(),
        pair_key: URL_SAFE_NO_PAD.encode([42; 32]),
        pairing_state_dir: None,
        api_key: "a".repeat(32),
        listen: "127.0.0.1:8787".parse().unwrap(),
        upstream: "http://127.0.0.1:8080".into(),
        upstream_socket: None,
        upstream_api_key: String::new(),
        relay: RelayPolicy::Disabled,
        ice_servers: vec![],
        request_timeout_seconds: 60,
        allow_unverified_backend: false,
        include_loopback_candidate: true,
        resume_seconds: 0,
        backends: vec![],
        max_concurrent_requests: 1,
        max_queued_requests: 0,
        management_token_file: None,
        model_control_socket: None,
    };
    config.validate().unwrap();
    let cfg = config.clone();
    // Host joins first and is therefore WebRTC OFFERER, independent of its API role.
    let home = tokio::spawn(async move { peer::accept(&cfg, &mut String::new()).await.unwrap() });
    while exchange.joins.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    let client = tokio::time::timeout(Duration::from_secs(10), peer::connect(&config))
        .await
        .expect("connection must not await a slow signaling acknowledgement")
        .unwrap();
    let mut home = tokio::time::timeout(Duration::from_secs(3), home)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(exchange.calls.load(Ordering::SeqCst), 5);
    tokio::time::timeout(Duration::from_secs(2), async {
        while active_connections.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("all signaling TCP connections must close as soon as the channel opens");
    client
        .tx
        .send(Frame::Request {
            id: 1,
            method: "GET".into(),
            path: "/v1/models".into(),
        })
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(2), home.rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(frame, Frame::Request { id: 1, .. }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(active_connections.load(Ordering::SeqCst), 0);
    assert_eq!(
        exchange.calls.load(Ordering::SeqCst),
        5,
        "no signaling poll after connection"
    );
    client.close().await;
    home.close().await;
    proxy.abort();
    server.abort();
}

struct ConnectionCount(Arc<AtomicUsize>);
impl Drop for ConnectionCount {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn connection_probe(
    target: std::net::SocketAddr,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let active = Arc::new(AtomicUsize::new(0));
    let counter = active.clone();
    let task = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let guard = ConnectionCount(counter.clone());
            tokio::spawn(async move {
                let _guard = guard;
                let upstream = tokio::net::TcpStream::connect(target).await.unwrap();
                let (mut client_read, mut client_write) = client.into_split();
                let (mut upstream_read, mut upstream_write) = upstream.into_split();
                // EOF in either direction closes this probe connection immediately.
                tokio::select! {
                    _ = tokio::io::copy(&mut client_read, &mut upstream_write) => {},
                    _ = tokio::io::copy(&mut upstream_read, &mut client_write) => {},
                }
            });
        }
    });
    (url, active, task)
}
