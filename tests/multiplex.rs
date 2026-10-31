use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use k_lani_link::{
    config::{self, Config, SignalingBackend},
    gateway::Gateway,
    host, signal,
};
#[cfg(unix)]
use std::sync::atomic::AtomicBool;
use std::{
    convert::Infallible,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Backend {
    model: Arc<Mutex<Option<String>>>,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    finish: Arc<tokio::sync::Notify>,
    #[cfg(unix)]
    controls: Arc<AtomicUsize>,
    #[cfg(unix)]
    overlap: Arc<AtomicBool>,
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn chat(State(state): State<Backend>, Json(body): Json<serde_json::Value>) -> Response {
    if state.model.lock().unwrap().as_deref() != body["model"].as_str() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error":"model mismatch"})),
        )
            .into_response();
    }
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.maximum.fetch_max(active, Ordering::SeqCst);
    let guard = Active(state.active.clone());
    let label = body["label"].as_str().unwrap().to_owned();
    let hold = body["hold"] == true;
    let burst = body["burst"] == true;
    let gated = body["gated"] == true;
    let finish = state.finish.clone();
    let stream = async_stream::stream! {
        let _hold = guard;
        yield Ok::<_,Infallible>(Bytes::from(format!("data: {{\"reasoning\":\"{label}\"}}\n\n")));
        if hold || burst { loop {
            if hold { tokio::time::sleep(Duration::from_millis(20)).await; }
            yield Ok(Bytes::from(if burst { "B".repeat(8192) } else { format!(": {label}\n\n") }));
        }}
        if gated { finish.notified().await; }
        tokio::time::sleep(Duration::from_millis(40)).await;
        yield Ok(Bytes::from(format!("data: {{\"tool\":\"{label}\",\"answer\":\"{label}\"}}\n\ndata: [DONE]\n\n")));
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
#[cfg(unix)]
async fn model_control(
    State(state): State<Backend>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    state.controls.fetch_add(1, Ordering::SeqCst);
    if state.active.load(Ordering::SeqCst) != 0 {
        state.overlap.store(true, Ordering::SeqCst);
    }
    tokio::time::sleep(Duration::from_millis(80)).await;
    *state.model.lock().unwrap() = if body["action"] == "unload" {
        None
    } else {
        body["model"].as_str().map(str::to_owned)
    };
    Json(serde_json::json!({"active_model":*state.model.lock().unwrap()}))
}
async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (base, task)
}
struct Lab {
    base: String,
    cfg: Config,
    gateway: Gateway,
    state: Backend,
    http: reqwest::Client,
    root: PathBuf,
    stop: CancellationToken,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Lab {
    fn drop(&mut self) {
        self.stop.cancel();
        for task in &self.tasks {
            task.abort();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl Lab {
    async fn new() -> Self {
        Self::with(2, 1, None).await
    }
    async fn with(concurrent: usize, queued: usize, upload_timeout: Option<Duration>) -> Self {
        let root = std::env::temp_dir().join(format!("kl-m-{}", config::random_id().unwrap()));
        std::fs::create_dir(&root).unwrap();
        let state = Backend {
            model: Arc::new(Mutex::new(Some("alpha".into()))),
            active: Arc::new(AtomicUsize::new(0)),
            maximum: Arc::new(AtomicUsize::new(0)),
            finish: Arc::new(tokio::sync::Notify::new()),
            #[cfg(unix)]
            controls: Arc::new(AtomicUsize::new(0)),
            #[cfg(unix)]
            overlap: Arc::new(AtomicBool::new(false)),
        };
        let (upstream, backend) = server(Router::new().route("/health",get(||async {Json(serde_json::json!({"link_privacy":{"version":1,"application_logs":false,"request_history":false,"diagnostics":false,"remote_images":false}}))}))
            .route("/v1/chat/completions",post(chat)).route("/v1/responses",post(chat)).with_state(state.clone())).await;
        let (url, signaling) = server(signal::router("s".repeat(32))).await;
        let pair = root.join("pair");
        config::generate_pair(&pair, &url, SignalingBackend::Mailbox, vec![]).unwrap();
        let mut cfg = Config::load(&pair.join("client.toml")).unwrap();
        cfg.signaling_token = "s".repeat(32);
        cfg.include_loopback_candidate = true;
        cfg.upstream = upstream;
        cfg.max_concurrent_requests = concurrent;
        cfg.max_queued_requests = queued;
        cfg.request_timeout_seconds = 10;
        let key = root.join("management-token");
        config::private_file(&key, &"m".repeat(43)).unwrap();
        cfg.management_token_file = Some(key);
        #[cfg(unix)]
        let control = {
            let socket = root.join("control.sock");
            cfg.model_control_socket = Some(socket.clone());
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let control_state = state.clone();
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new()
                        .route("/v1/link/model", post(model_control))
                        .route(
                            "/v1/link/models",
                            get(|| async {
                                Json(serde_json::json!({"models":[{"id":"alpha"},{"id":"beta"}]}))
                            }),
                        )
                        .with_state(control_state),
                )
                .await
                .unwrap();
            })
        };
        let stop = CancellationToken::new();
        let host_cfg = cfg.clone();
        let host_stop = stop.clone();
        let host_dir = root.join("host-state");
        let host = tokio::spawn(async move {
            host::run(host_cfg, host_stop, host_dir).await.unwrap();
        });
        let mut gateway = Gateway::new(cfg.clone()).unwrap();
        if let Some(timeout) = upload_timeout {
            gateway = gateway.with_upload_timeout(timeout);
        }
        let link = gateway.clone();
        let client_stop = stop.clone();
        let client_dir = root.join("client-state");
        let client = tokio::spawn(async move {
            link.maintain(client_stop, client_dir).await;
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !gateway.is_connected() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let used = ["host-state", "client-state"]
                    .iter()
                    .filter(|name| {
                        std::fs::read_dir(root.join(name)).is_ok_and(|entries| {
                            entries
                                .flatten()
                                .any(|e| e.path().extension().is_some_and(|x| x == "used"))
                        })
                    })
                    .count();
                if used == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        signaling.abort();
        let (base, api) = server(gateway.router()).await;
        let tasks = vec![backend, host, client, api];
        #[cfg(unix)]
        let tasks = {
            let mut tasks = tasks;
            tasks.push(control);
            tasks
        };
        Self {
            base,
            cfg,
            gateway,
            state,
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(8))
                .build()
                .unwrap(),
            root,
            stop,
            tasks,
        }
    }
    fn chat(&self, model: &str, label: &str, hold: bool) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}/v1/chat/completions", self.base))
            .bearer_auth(&self.cfg.api_key)
            .json(&serde_json::json!({"model":model,"label":label,"hold":hold,"stream":true}))
    }
    #[cfg(unix)]
    fn control(&self, body: serde_json::Value) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}/v1/link/model", self.base))
            .bearer_auth("m".repeat(43))
            .json(&body)
    }
    async fn idle(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.state.active.load(Ordering::SeqCst) != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[cfg(unix)]
    fn used(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for name in ["host-state", "client-state"] {
            for entry in std::fs::read_dir(self.root.join(name)).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "used") {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        paths
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_streams_queue_cancel_and_overload_on_one_authenticated_tunnel() {
    let lab = Lab::new().await;
    let first = lab.chat("alpha", "FIRST", true).send().await.unwrap();
    let second = lab.chat("alpha", "SECOND", true).send().await.unwrap();
    assert_eq!(lab.state.active.load(Ordering::SeqCst), 2);
    let request = lab.chat("alpha", "QUEUED", false);
    let queued = tokio::spawn(async move { request.send().await.unwrap().text().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!queued.is_finished());
    assert_eq!(
        lab.chat("alpha", "OVERFLOW", false)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(lab.state.maximum.load(Ordering::SeqCst), 2);
    drop(first);
    let text = queued.await.unwrap();
    assert!(text.contains("QUEUED") && !text.contains("FIRST") && !text.contains("SECOND"));
    let mut survivor = second.bytes_stream();
    assert!(survivor.next().await.unwrap().is_ok());
    drop(survivor);
    lab.idle().await;
    // HTTP cancellation closes the forwarding task. The server can observe the
    // disconnect later; that cleanup delay is not an extra admitted live stream.
    assert!(lab.gateway.is_connected());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_switch_drains_inference_preserves_pairing_and_requires_separate_operator_key() {
    let lab = Lab::new().await;
    let used = lab.used();
    assert_eq!(used.len(), 2);
    assert_eq!(
        lab.http
            .post(format!("{}/v1/link/model", lab.base))
            .bearer_auth(&lab.cfg.api_key)
            .json(&serde_json::json!({"action":"unload"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for body in [
        serde_json::json!({"action":"load","model":"../../secret"}),
        serde_json::json!({"action":"load","model":"beta","path":"/tmp/x"}),
        serde_json::json!({}),
    ] {
        assert_eq!(
            lab.control(body).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(lab.state.controls.load(Ordering::SeqCst), 0);
    let running = lab
        .chat("alpha", "OLD", false)
        .json(&serde_json::json!({"model":"alpha","label":"OLD","gated":true,"stream":true}))
        .send()
        .await
        .unwrap();
    let request = lab.control(serde_json::json!({"action":"load","model":"beta"}));
    let switching = tokio::spawn(async move {
        request
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(!switching.is_finished());
    let request = lab.chat("beta", "NEW", false);
    let after = tokio::spawn(async move { request.send().await.unwrap().text().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!after.is_finished());
    // Prove the barrier waits for normal completion, independently of when an
    // HTTP server notices a cancelled socket and releases its own resources.
    lab.state.finish.notify_one();
    assert!(running.text().await.unwrap().contains("[DONE]"));
    assert_eq!(switching.await.unwrap()["active_model"], "beta");
    let text = after.await.unwrap();
    assert!(text.contains("NEW") && !text.contains("OLD"));
    assert!(!lab.state.overlap.load(Ordering::SeqCst));
    assert!(lab.gateway.is_connected());
    assert_eq!(lab.used(), used);
    assert!(
        lab.control(serde_json::json!({"action":"unload"}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        lab.control(serde_json::json!({"action":"load","model":"alpha"}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    assert!(
        lab.chat("alpha", "AGAIN", false)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
            .contains("AGAIN")
    );
    assert_eq!(lab.used(), used);
    assert!(lab.gateway.is_connected());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unread_stream_cannot_block_another_stream() {
    let lab = Lab::new().await;
    let slow = lab
        .http
        .post(format!("{}/v1/responses", lab.base))
        .bearer_auth(&lab.cfg.api_key)
        .json(&serde_json::json!({"model":"alpha","label":"SLOW","burst":true}))
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    let text = tokio::time::timeout(Duration::from_secs(3), async {
        lab.chat("alpha", "FAST", false)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert!(text.contains("FAST") && !text.contains("SLOW"));
    drop(slow);
    lab.idle().await;
    assert!(lab.gateway.is_connected());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_upload_gives_its_slot_back_after_the_upload_limit() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // One slot, no queue, and a request timeout of ten seconds: before the upload limit existed,
    // a client that stopped sending its body kept the only slot for those ten seconds.
    let lab = Lab::with(1, 0, Some(Duration::from_millis(300))).await;
    let address = lab.base.trim_start_matches("http://").to_owned();
    let mut stalled = tokio::net::TcpStream::connect(&address).await.unwrap();
    let head = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: x\r\nauthorization: Bearer {}\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{{\"mo",
        lab.cfg.api_key
    );
    stalled.write_all(head.as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // While the body is still expected, the slot is taken.
    assert_eq!(
        lab.chat("alpha", "OTHER", false)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // The upload limit ends the stalled request with 408 long before the request timeout.
    let mut reply = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(3), stalled.read_exact(&mut reply))
        .await
        .expect("a stalled upload must end at the upload limit")
        .unwrap();
    assert_eq!(&reply, b"HTTP/1.1 408");
    let text = lab
        .chat("alpha", "AFTER", false)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.contains("AFTER"));
    assert!(lab.gateway.is_connected());
}
