use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use k_lani_link::{
    config::{Config, RelayPolicy},
    gateway::Gateway,
    host, peer, signal,
};
use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Default)]
struct Mock {
    body: Arc<Mutex<serde_json::Value>>,
    stopped: Arc<AtomicUsize>,
    private: Arc<AtomicBool>,
}
async fn mock_health(State(state): State<Mock>) -> Json<serde_json::Value> {
    let privacy = state.private.load(Ordering::SeqCst).then(|| serde_json::json!({
        "version":1,"application_logs":false,"request_history":false,"diagnostics":false,"remote_images":false
    }));
    Json(serde_json::json!({"link_privacy":privacy}))
}
struct ObserveDrop(Arc<AtomicUsize>);
impl Drop for ObserveDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
async fn mock_chat(State(state): State<Mock>, Json(body): Json<serde_json::Value>) -> Response {
    *state.body.lock().unwrap() = body.clone();
    if body["model"] == "missing" {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":{"message":"missing model"}})),
        )
            .into_response();
    }
    if body["stream"] != true {
        return Json(serde_json::json!({"choices":[{"message":{"content":"complete"}}]}))
            .into_response();
    }
    let guard = ObserveDrop(state.stopped.clone());
    let infinite = body["model"] == "cancel";
    let stream = async_stream::stream! {
        let _guard = guard;
        yield Ok::<_, Infallible>(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"checking the code\"}}]}\n\n"));
        tokio::time::sleep(Duration::from_millis(100)).await;
        yield Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\"}}]}}]}\n\n"));
        if infinite {
            loop { tokio::time::sleep(Duration::from_millis(20)).await; yield Ok(Bytes::from_static(b": keepalive\n\n")); }
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
        yield Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hallo\"}}]}\n\ndata: [DONE]\n\n"));
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (base, task)
}
fn config(signaling_url: String, upstream: String) -> Config {
    Config {
        transport: Default::default(),
        https: None,
        signaling_url,
        signaling_backend: Default::default(),
        signaling_token: "s".repeat(32),
        room: k_lani_link_protocol::room_from_key(&URL_SAFE_NO_PAD.encode([42; 32])).unwrap(),
        pair_key: URL_SAFE_NO_PAD.encode([42; 32]),
        pairing_state_dir: None,
        api_key: "a".repeat(32),
        listen: "127.0.0.1:8787".parse().unwrap(),
        upstream,
        upstream_socket: None,
        upstream_api_key: String::new(),
        relay: RelayPolicy::Disabled,
        ice_servers: vec![],
        request_timeout_seconds: 30,
        allow_unverified_backend: false,
        include_loopback_candidate: true,
        resume_seconds: 0,
        backends: vec![],
        max_concurrent_requests: 1,
        max_queued_requests: 0,
        management_token_file: None,
        model_control_socket: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_webrtc_streams_tools_after_signaling_shutdown_and_cancels_upstream() {
    let state = Mock::default();
    state.private.store(true, Ordering::SeqCst);
    let mock = Router::new()
        .route("/health", get(mock_health))
        .route(
            "/v1/models",
            get(|| async { Json(serde_json::json!({"data":[{"id":"basalt-test"}]})) }),
        )
        .route("/v1/chat/completions", post(mock_chat))
        .route("/v1/responses", post(mock_chat))
        .with_state(state.clone());
    let (upstream, upstream_task) = server(mock).await;
    let (signal_url, signal_task) = server(signal::router("s".repeat(32))).await;
    let cfg = config(signal_url, upstream);
    cfg.validate().unwrap();
    let cfg_host = cfg.clone();
    let host_setup =
        tokio::spawn(async move { peer::accept(&cfg_host, &mut String::new()).await.unwrap() });
    let client_peer = peer::connect(&cfg).await.unwrap();
    let host_peer = host_setup.await.unwrap();
    let peer_shutdown = client_peer.closed.clone();
    let cfg_host = cfg.clone();
    let host_task = tokio::spawn(async move {
        host::serve(&cfg_host, host_peer).await.unwrap();
    });
    let gateway = Gateway::new(cfg.clone()).unwrap();
    let attach = gateway.clone();
    let attach_task = tokio::spawn(async move {
        attach.attach(client_peer).await;
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !gateway.is_connected() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (base, api_task) = server(gateway.router()).await;

    // Actual rendezvous process disappears BEFORE any inference request.
    signal_task.abort();
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert_eq!(
        http.get(format!("{base}/v1/models"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let models: serde_json::Value = http
        .get(format!("{base}/v1/models"))
        .bearer_auth(&cfg.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "basalt-test");
    // A 256K coding context can exceed the old 1 MiB JSON limit even without images.
    let large = serde_json::json!({"model":"basalt-test","messages":[{"role":"user","content":"x".repeat(1536 * 1024)}]});
    let response = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&large)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    assert_eq!(*state.body.lock().unwrap(), large);
    let too_large = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .body(vec![b'x'; k_lani_link_protocol::MAX_BODY_BYTES + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        *state.body.lock().unwrap(),
        large,
        "oversized input must never reach inference"
    );
    let request = serde_json::json!({"model":"basalt-test","stream":true,"messages":[{"role":"user","content":"hello"}],"tools":[{"type":"function","function":{"name":"read_file","parameters":{"type":"object"}}}]});
    let response = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut stream = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("first chunk must arrive before completion")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("reasoning_content"));
    assert!(!String::from_utf8_lossy(&first).contains("[DONE]"));
    assert_eq!(*state.body.lock().unwrap(), request);
    assert_eq!(
        http.get(format!("{base}/v1/models"))
            .bearer_auth(&cfg.api_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let mut remaining = Vec::new();
    while let Some(bytes) = stream.next().await {
        remaining.extend_from_slice(&bytes.unwrap());
    }
    assert!(String::from_utf8_lossy(&remaining).contains("data: [DONE]"));
    assert!(String::from_utf8_lossy(&remaining).contains("tool_calls"));
    drop(stream);

    let response = http
        .post(format!("{base}/v1/responses"))
        .bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"test","input":"hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.text().await.unwrap().contains("complete"));
    let response = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"missing"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    response.bytes().await.unwrap();
    assert_eq!(
        http.post(format!("{base}/v1/load"))
            .bearer_auth(&cfg.api_key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    // A backend restarted without its privacy profile must receive no new prompt.
    state.private.store(false, Ordering::SeqCst);
    let previous = state.body.lock().unwrap().clone();
    let denied = http.post(format!("{base}/v1/chat/completions")).bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"private","messages":[{"role":"user","content":"do-not-forward"}]}))
        .send().await.unwrap();
    assert_eq!(denied.status(), StatusCode::BAD_GATEWAY);
    denied.bytes().await.unwrap();
    assert_eq!(*state.body.lock().unwrap(), previous);
    state.private.store(true, Ordering::SeqCst);
    let stopped = state.stopped.load(Ordering::SeqCst);
    let response = http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"cancel","stream":true}))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    stream.next().await.unwrap().unwrap();
    drop(stream);
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.stopped.load(Ordering::SeqCst) == stopped {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("HTTP disconnect must cancel home inference");

    peer_shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(3), attach_task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), host_task)
        .await
        .unwrap()
        .unwrap();
    api_task.abort();
    upstream_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mismatched_pair_key_does_not_establish_a_peer() {
    let (url, signal_task) = server(signal::router("s".repeat(32))).await;
    let mut cfg = config(url, "http://127.0.0.1:8080".into());
    let client_cfg = cfg.clone();
    let client = tokio::spawn(async move { peer::connect(&client_cfg).await });
    let mut wrong = [42; 32];
    wrong[31] = 43;
    cfg.pair_key = URL_SAFE_NO_PAD.encode(wrong);
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        peer::accept(&cfg, &mut String::new()),
    )
    .await
    .unwrap();
    assert!(result.err().unwrap().to_string().contains("authenticated"));
    client.abort();
    signal_task.abort();
}

#[tokio::test]
async fn direct_only_peer_rejects_remote_relay_before_negotiation() {
    let cfg = config(
        "http://127.0.0.1:8788".into(),
        "http://127.0.0.1:8080".into(),
    );
    let peer = peer::Peer::new(&cfg, false).await.unwrap();
    let sdp =
        "v=0\r\na=candidate:1 1 udp 1 192.0.2.1 3478 typ relay raddr 192.0.2.2 rport 1234\r\n";
    let error = peer
        .set_remote(sdp.into(), true, RelayPolicy::Disabled)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("relay is disabled"));
    peer.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authentication_failure_stops_pairing_without_consuming_the_code() {
    use k_lani_link::{
        pairing::{PairingLease, PairingRejected},
        signal::Signaling,
    };
    use k_lani_link_protocol::EndpointRole;
    use tokio_util::sync::CancellationToken;

    let (url, signal_task) = server(signal::router("s".repeat(32))).await;
    let cfg = config(url, "http://127.0.0.1:8080".into());
    let root = std::env::temp_dir().join(format!(
        "link-reject-{}",
        k_lani_link::config::random_id().unwrap()
    ));
    let mut wrong = cfg.clone();
    let mut bytes = [42; 32];
    bytes[31] = 43; // Same public room prefix, different secret.
    wrong.pair_key = URL_SAFE_NO_PAD.encode(bytes);
    let signaling = Signaling::new(&cfg).unwrap();
    let offer = signal::seal(
        &wrong,
        "rejected-session",
        "offer",
        "v=0\r\n",
        EndpointRole::Client,
    )
    .unwrap();
    signaling.put(&offer).await.unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        host::run(cfg.clone(), CancellationToken::new(), root.join("host")),
    )
    .await
    .unwrap();
    assert!(
        result.err().unwrap().is::<PairingRejected>(),
        "host must stop, not retry authentication failures"
    );
    drop(PairingLease::reserve(&root.join("host"), &cfg.pair_key).unwrap());

    let gateway = Gateway::new(cfg.clone()).unwrap();
    let background = gateway.clone();
    let state_dir = root.join("client");
    let maintain = tokio::spawn(async move {
        background
            .maintain(CancellationToken::new(), state_dir)
            .await;
    });
    let offer = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(offer) = signaling.get("offer").await.unwrap()
                && offer.context.session != "rejected-session"
            {
                break offer;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let answer = signal::seal(
        &wrong,
        &offer.context.session,
        "answer",
        "v=0\r\n",
        EndpointRole::Host,
    )
    .unwrap();
    signaling.put(&answer).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), maintain)
        .await
        .expect("client must stop, not retry authentication failures")
        .unwrap();
    drop(PairingLease::reserve(&root.join("client"), &cfg.pair_key).unwrap());
    assert!(!gateway.is_connected());
    let (api, api_task) = server(gateway.router()).await;
    let health: serde_json::Value = reqwest::Client::new()
        .get(format!("{api}/health"))
        .bearer_auth(&cfg.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["pairing_rejected"], true);
    assert_eq!(health["new_code_required"], false);
    api_task.abort();
    signal_task.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_broker_messages_are_terminal_in_both_backends() {
    use k_lani_link::{config::SignalingBackend, pairing::PairingRejected};
    let (url, task) = server(Router::new().fallback(|| async { "invalid-json" })).await;
    let mut cfg = config(url, "http://127.0.0.1:8080".into());
    for backend in [SignalingBackend::Mailbox, SignalingBackend::IceExchange] {
        cfg.signaling_backend = backend;
        let client = tokio::time::timeout(Duration::from_secs(5), peer::connect(&cfg))
            .await
            .unwrap();
        assert!(client.err().unwrap().is::<PairingRejected>());
        let host = tokio::time::timeout(
            Duration::from_secs(5),
            peer::accept(&cfg, &mut String::new()),
        )
        .await
        .unwrap();
        assert!(host.err().unwrap().is::<PairingRejected>());
    }
    task.abort();
}

/// Pairs a host and a gateway through `cfg`'s rendezvous. Returns the gateway, the handle that
/// closes the tunnel, and the two tasks that end when it closes.
async fn connect_gateway(
    cfg: &Config,
) -> (
    Gateway,
    tokio_util::sync::CancellationToken,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
) {
    let host_cfg = cfg.clone();
    let host_setup =
        tokio::spawn(async move { peer::accept(&host_cfg, &mut String::new()).await.unwrap() });
    let client_peer = peer::connect(cfg).await.unwrap();
    let host_peer = host_setup.await.unwrap();
    let closed = client_peer.closed.clone();
    let host_cfg = cfg.clone();
    let host_task = tokio::spawn(async move {
        host::serve(&host_cfg, host_peer).await.unwrap();
    });
    let gateway = Gateway::new(cfg.clone()).unwrap();
    let attach = gateway.clone();
    let attach_task = tokio::spawn(async move { attach.attach(client_peer).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !gateway.is_connected() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    (gateway, closed, host_task, attach_task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backend_without_privacy_profile_is_used_only_after_explicit_opt_out() {
    // The backend never reports a privacy profile, e.g. an unpatched general-purpose server.
    let state = Mock::default();
    let mock = Router::new()
        .route("/health", get(mock_health))
        .route("/v1/chat/completions", post(mock_chat))
        .with_state(state.clone());
    let (upstream, upstream_task) = server(mock).await;
    for (allow, expected) in [(false, StatusCode::BAD_GATEWAY), (true, StatusCode::OK)] {
        let (signal_url, signal_task) = server(signal::router("s".repeat(32))).await;
        let mut cfg = config(signal_url, upstream.clone());
        cfg.allow_unverified_backend = allow;
        let (gateway, closed, host_task, attach_task) = connect_gateway(&cfg).await;
        let (base, api_task) = server(gateway.router()).await;
        *state.body.lock().unwrap() = serde_json::Value::Null;
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&cfg.api_key)
            .json(&serde_json::json!({"model":"m","messages":[{"role":"user","content":"prompt"}]}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            expected,
            "allow_unverified_backend = {allow}"
        );
        response.bytes().await.unwrap();
        assert_eq!(
            state.body.lock().unwrap().is_null(),
            !allow,
            "the prompt may reach an unverified backend only after the explicit opt-out"
        );
        closed.cancel();
        tokio::time::timeout(Duration::from_secs(3), attach_task)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), host_task)
            .await
            .unwrap()
            .unwrap();
        api_task.abort();
        signal_task.abort();
    }
    upstream_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_request_does_not_wait_for_a_running_generation() {
    use k_lani_link::shutdown::with_grace;
    use tokio_util::sync::CancellationToken;

    let state = Mock::default();
    state.private.store(true, Ordering::SeqCst);
    let mock = Router::new()
        .route("/health", get(mock_health))
        .route("/v1/chat/completions", post(mock_chat))
        .with_state(state.clone());
    let (upstream, upstream_task) = server(mock).await;
    let (signal_url, signal_task) = server(signal::router("s".repeat(32))).await;
    let cfg = config(signal_url, upstream);
    let (gateway, _closed, host_task, attach_task) = connect_gateway(&cfg).await;

    // Serve the API the way `client` does, with a short grace period for the test.
    let stop = CancellationToken::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = {
        let stop = stop.clone();
        let router = gateway.router();
        tokio::spawn(async move {
            let server = axum::serve(listener, router)
                .with_graceful_shutdown(stop.clone().cancelled_owned());
            with_grace(
                &stop,
                Duration::from_millis(400),
                std::future::IntoFuture::into_future(server),
            )
            .await
        })
    };

    // The mock keeps this generation running until the HTTP client goes away.
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth(&cfg.api_key)
        .json(&serde_json::json!({"model":"cancel","stream":true}))
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    stream.next().await.unwrap().unwrap();

    stop.cancel();
    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .expect("shutdown must not wait for the running generation")
        .unwrap();
    assert!(
        outcome.is_none(),
        "the grace period, not the stream, ended the server"
    );
    assert!(started.elapsed() < Duration::from_secs(3));

    drop(stream);
    host_task.abort();
    attach_task.abort();
    upstream_task.abort();
    signal_task.abort();
}
