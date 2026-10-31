use super::*;
use crate::{
    chain_state::Lifetimes,
    config::{private_file, random_secret},
};
use k_lani_link_protocol::{Chain, decode_data};
use std::path::PathBuf;

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Shutdown(CancellationToken);
impl Drop for Shutdown {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

struct Fault {
    mode: u8,
    enabled: AtomicBool,
    claimed: AtomicBool,
    published: std::sync::atomic::AtomicUsize,
    gate: tokio::sync::Semaphore,
    cursors: Mutex<Vec<u64>>,
    put_hashes: Mutex<Vec<[u8; 32]>>,
    saved_reply: Mutex<Option<Bytes>>,
    hellos: std::sync::atomic::AtomicUsize,
    quic_requests: std::sync::atomic::AtomicUsize,
}
impl Fault {
    fn new(mode: u8) -> Self {
        Self {
            mode,
            enabled: AtomicBool::new(true),
            claimed: AtomicBool::new(false),
            published: Default::default(),
            gate: tokio::sync::Semaphore::new(0),
            cursors: Mutex::new(Vec::new()),
            put_hashes: Mutex::new(Vec::new()),
            saved_reply: Mutex::new(None),
            hellos: Default::default(),
            quic_requests: Default::default(),
        }
    }
}
async fn intercept(
    axum::extract::State(fault): axum::extract::State<Arc<Fault>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::{
        body::{Body, to_bytes},
        http::{Method, StatusCode},
        response::IntoResponse,
    };
    use sha2::{Digest, Sha256};
    let path = request.uri().path().to_owned();
    if request.version() == axum::http::Version::HTTP_3 {
        fault.quic_requests.fetch_add(1, Ordering::SeqCst);
    }
    if fault.mode == 9 {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "https://localhost:9/")],
        )
            .into_response();
    }
    if fault.mode == 10 {
        // A buffering proxy waits for each complete finite request and response body.
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, MAX_PACKET_BYTES).await.unwrap();
        let response = next
            .run(axum::extract::Request::from_parts(parts, Body::from(bytes)))
            .await;
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, WINDOW * (MAX_PACKET_BYTES + 4))
            .await
            .unwrap();
        return axum::response::Response::from_parts(parts, Body::from(bytes));
    }
    let from = url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
        .find(|(k, _)| k == "from")
        .and_then(|(_, v)| v.parse::<u64>().ok())
        .unwrap_or(0);
    let get = request.method() == Method::GET && path.ends_with("/h2c") && from >= 2;
    let put = request.method() == Method::PUT && path.contains("/h2c/") && !path.ends_with("/1");
    if get {
        fault.cursors.lock().unwrap().push(from);
    }
    let reply = request.method() == Method::GET
        && path.starts_with("/v1/hello/")
        && path.contains("/host/");
    if request.method() == Method::PUT
        && path.starts_with("/v1/hello/")
        && path.ends_with("/client")
    {
        fault.hellos.fetch_add(1, Ordering::SeqCst);
    }
    // A web host or firewall that only permits GET and POST.
    // Like OWASP CRS defaults: no PUT, and no request body other than JSON or forms.
    let binary = request
        .headers()
        .get("content-type")
        .is_some_and(|value| value != "application/json");
    if fault.mode == 13 && (request.method() == Method::PUT || binary) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if fault.mode == 12 && request.method() == Method::PUT {
        // Web application firewalls usually answer a refused method with 403.
        return StatusCode::FORBIDDEN.into_response();
    }
    if get && fault.mode == 7 && fault.enabled.load(Ordering::SeqCst) {
        return StatusCode::GONE.into_response();
    }
    // The relay stores the hello, but every answer to that PUT is lost on the way back.
    if fault.mode == 14 && request.method() == Method::PUT && path.ends_with("/client") {
        let _ = next.run(request).await;
        return StatusCode::BAD_GATEWAY.into_response();
    }
    if reply && fault.mode == 11 && fault.enabled.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if get && fault.mode == 5 && fault.enabled.load(Ordering::SeqCst) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let selected = get
        && (1..=4).contains(&fault.mode)
        && fault.enabled.load(Ordering::SeqCst)
        && !fault.claimed.swap(true, Ordering::SeqCst);
    if selected {
        fault.gate.acquire().await.unwrap().forget();
    }
    if fault.mode == 6 && request.method() == Method::PUT && path.ends_with("/c2h/2") {
        let (parts, body) = request.into_parts();
        let bytes = to_bytes(body, MAX_PACKET_BYTES).await.unwrap();
        fault
            .put_hashes
            .lock()
            .unwrap()
            .push(Sha256::digest(&bytes).into());
        let response = next
            .run(axum::extract::Request::from_parts(parts, Body::from(bytes)))
            .await;
        if response.status() == StatusCode::NO_CONTENT
            && !fault.claimed.swap(true, Ordering::SeqCst)
        {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        return response;
    }
    let response = next.run(request).await;
    if reply && fault.mode == 8 && response.status() == StatusCode::OK {
        let (mut parts, body) = response.into_parts();
        let bytes = to_bytes(body, REPLY_BYTES).await.unwrap();
        let mut saved = fault.saved_reply.lock().unwrap();
        let selected = if let Some(previous) = saved.as_ref() {
            previous.clone()
        } else {
            *saved = Some(bytes.clone());
            bytes
        };
        parts.headers.insert(
            "content-length",
            selected.len().to_string().parse().unwrap(),
        );
        return axum::response::Response::from_parts(parts, Body::from(selected));
    }
    if put && response.status() == StatusCode::NO_CONTENT {
        fault.published.fetch_add(1, Ordering::SeqCst);
    }
    if !selected || response.status() != StatusCode::OK {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let bytes = to_bytes(body, WINDOW * (MAX_PACKET_BYTES + 4))
        .await
        .unwrap();
    let packets = packets(&bytes).unwrap();
    assert!(packets.len() >= 3);
    let prefixed = |packet: &Bytes| {
        let mut out = Vec::new();
        out.extend_from_slice(&(packet.len() as u32).to_be_bytes());
        out.extend_from_slice(packet);
        out
    };
    let mut out = Vec::new();
    match fault.mode {
        1 => {
            out.extend_from_slice(&bytes);
            out[4 + packets[0].len() - 1] ^= 1;
        }
        2 => {
            out.extend_from_slice(&prefixed(&packets[1]));
            out.extend_from_slice(&prefixed(&packets[0]));
            for packet in &packets[2..] {
                out.extend_from_slice(&prefixed(packet));
            }
        }
        3 => {
            out.extend_from_slice(&prefixed(&packets[0]));
            out.extend_from_slice(&bytes);
        }
        4 => {
            out.extend_from_slice(&prefixed(&packets[0]));
            out.extend_from_slice(&[0, 0, 0]);
            let stream = async_stream::stream! {
                yield Ok(Bytes::from(out));
                tokio::time::sleep(Duration::from_millis(40)).await;
                yield Err::<Bytes,_>(std::io::Error::other("synthetic response cut"));
            };
            return axum::response::Response::from_parts(parts, Body::from_stream(stream));
        }
        _ => unreachable!(),
    }
    parts
        .headers
        .insert("content-length", out.len().to_string().parse().unwrap());
    axum::response::Response::from_parts(parts, Body::from(out))
}
struct Fixture {
    shutdown: Shutdown,
    server: JoinHandle<()>,
    options: Options,
    host: SharedStore,
    client: SharedStore,
    _relay: Arc<super::super::relay::Relay>,
    fault: Arc<Fault>,
    tls: axum_server::tls_rustls::RustlsConfig,
    /// Applies from the next `restart_relay`.
    relay_http1: bool,
    _cleanup: Cleanup,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_fault(0).await
    }
    async fn with_fault(mode: u8) -> Self {
        let root = std::env::temp_dir().join(format!(
            "link-https-{}",
            crate::config::random_id().unwrap()
        ));
        crate::pairing::private_directory(&root).unwrap();
        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            certificate.cert.pem().into_bytes(),
            certificate.signing_key.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let token = random_secret().unwrap();
        let relay = super::super::relay::Relay::new(
            super::super::relay::Limits::default(),
            vec![(token.clone(), super::super::relay::Limits::default())],
        )
        .unwrap();
        let stop = CancellationToken::new();
        let fault = Arc::new(Fault::new(mode));
        let router = relay
            .router(stop.clone())
            .layer(axum::middleware::from_fn_with_state(
                fault.clone(),
                intercept,
            ));
        let stopping = stop.clone();
        let serving_tls = tls.clone();
        let server = tokio::spawn(async move {
            super::super::server::serve_router(
                listener,
                router,
                Some(serving_tls),
                256,
                false,
                stopping,
            )
            .await
            .unwrap();
        });
        let chain = Chain::from_code(&random_secret().unwrap()).unwrap();
        let mut host = Store::open(&root.join("host"), EndpointRole::Host).unwrap();
        let mut client = Store::open(&root.join("client"), EndpointRole::Client).unwrap();
        host.initialize(&chain, now(), Lifetimes::default())
            .unwrap();
        client
            .initialize(&chain, now(), Lifetimes::default())
            .unwrap();
        private_file(&root.join("token"), &token).unwrap();
        private_file(&root.join("ca.pem"), &certificate.cert.pem()).unwrap();
        let options = Options {
            relays: vec![format!("https://localhost:{}", address.port())],
            state_dir: root.join("client"),
            admission_token_file: root.join("token"),
            proxy_url: None,
            ca_cert_file: Some(root.join("ca.pem")),
            poll_wait_seconds: 25,
            http1_only: false,
            http3: false,
            post_writes: false,
        };
        Self {
            shutdown: Shutdown(stop),
            server,
            options,
            host: Arc::new(Mutex::new(host)),
            client: Arc::new(Mutex::new(client)),
            _relay: relay,
            fault,
            tls,
            relay_http1: false,
            _cleanup: Cleanup(root),
        }
    }
    async fn pair(&self) -> (HttpsTunnel, HttpsTunnel) {
        let stop = CancellationToken::new();
        let (client, host) = tokio::join!(
            connect(&self.options, self.client.clone(), stop.clone()),
            accept(&self.options, self.host.clone(), stop)
        );
        (client.unwrap(), host.unwrap())
    }
    /// Restarts the actual relay on the same port; it keeps no session state.
    async fn restart_relay(&mut self) {
        self.shutdown.0.cancel();
        (&mut self.server).await.unwrap();
        let port = url::Url::parse(&self.options.relays[0])
            .unwrap()
            .port()
            .unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let relay = super::super::relay::Relay::new(
            super::super::relay::Limits::default(),
            vec![(
                super::super::secret(&self.options.admission_token_file).unwrap(),
                super::super::relay::Limits::default(),
            )],
        )
        .unwrap();
        let stop = CancellationToken::new();
        let router = relay
            .router(stop.clone())
            .layer(axum::middleware::from_fn_with_state(
                self.fault.clone(),
                intercept,
            ));
        let tls = self.tls.clone();
        let http1_only = self.relay_http1;
        let stopping = stop.clone();
        self.shutdown = Shutdown(stop);
        self.server = tokio::spawn(async move {
            super::super::server::serve_router(
                listener,
                router,
                Some(tls),
                256,
                http1_only,
                stopping,
            )
            .await
            .unwrap()
        });
    }
    /// Serves the same relay and fault layer over HTTP/3 on the TCP port's UDP twin.
    fn start_quic(&self) -> CancellationToken {
        let stop = CancellationToken::new();
        let port = url::Url::parse(&self.options.relays[0])
            .unwrap()
            .port()
            .unwrap();
        let endpoint = super::super::quic::listen(
            (std::net::Ipv4Addr::LOCALHOST, port).into(),
            &self.tls.get_inner(),
        )
        .unwrap();
        let router = self
            ._relay
            .router(stop.clone())
            .layer(axum::middleware::from_fn_with_state(
                self.fault.clone(),
                intercept,
            ));
        tokio::spawn(super::super::quic::serve(
            endpoint,
            router,
            64,
            stop.clone(),
        ));
        stop
    }
    async fn finish(self) {
        self.shutdown.0.cancel();
        self.server.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarting_the_actual_relay_ends_old_streams_and_only_explicit_connect_creates_a_new_session()
 {
    let mut fixture = Fixture::new().await;
    let (mut client, mut host) = fixture.pair().await;
    let current = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    let hello_count = fixture.fault.hellos.load(Ordering::SeqCst);
    fixture.restart_relay().await;
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(client.closed.cancelled(), host.closed.cancelled());
    })
    .await
    .unwrap();
    client.close().await;
    host.close().await;
    assert_eq!(fixture.fault.hellos.load(Ordering::SeqCst), hello_count);
    assert_eq!(
        fixture
            .client
            .lock()
            .unwrap()
            .client(now())
            .unwrap()
            .chain
            .room(),
        current
    );
    assert!(!client.rejected.load(Ordering::SeqCst) && !host.rejected.load(Ordering::SeqCst));
    let (mut client, mut host) = fixture.pair().await;
    client.tx.send(Frame::Credit { id: 73 }).await.unwrap();
    assert!(matches!(receive(&mut host).await, Frame::Credit { id: 73 }));
    assert_ne!(
        fixture
            .client
            .lock()
            .unwrap()
            .client(now())
            .unwrap()
            .chain
            .room(),
        current
    );
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hostile_box_tampering_reordering_and_duplication_end_the_session_without_duplicate_delivery()
 {
    for mode in 1..=3 {
        let fixture = Fixture::with_fault(mode).await;
        let (mut client, mut host) = fixture.pair().await;
        for n in 1..=3 {
            host.tx.send(Frame::data(n, &[n as u8])).await.unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while fixture.fault.published.load(Ordering::SeqCst) < n as usize {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        }
        fixture.fault.gate.add_permits(1);
        tokio::time::timeout(Duration::from_secs(3), client.closed.cancelled())
            .await
            .unwrap();
        assert!(client.rejected.load(Ordering::SeqCst));
        let mut delivered = Vec::new();
        while let Ok(frame) = client.rx.try_recv() {
            delivered.push(frame.id());
        }
        assert_eq!(delivered, if mode == 3 { vec![1] } else { Vec::new() });
        client.close().await;
        host.close().await;
        fixture.finish().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn truncated_multi_record_response_resumes_after_the_last_verified_record() {
    let fixture = Fixture::with_fault(4).await;
    let (mut client, mut host) = fixture.pair().await;
    for n in 1..=3 {
        host.tx.send(Frame::data(n, &[n as u8])).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture.fault.published.load(Ordering::SeqCst) < n as usize {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    fixture.fault.gate.add_permits(1);
    for n in 1..=3 {
        let Frame::Data { id, data } = receive(&mut client).await else {
            panic!()
        };
        assert_eq!(id, n);
        assert_eq!(decode_data(&data).unwrap(), vec![n as u8]);
    }
    assert!(fixture.fault.cursors.lock().unwrap().contains(&3));
    assert!(!client.closed.is_cancelled() && !client.rejected.load(Ordering::SeqCst));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http_success_without_authenticated_peer_records_does_not_extend_the_23_second_deadline() {
    let fixture = Fixture::with_fault(5).await;
    let (mut client, mut host) = fixture.pair().await;
    let chain = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    tokio::time::timeout(Duration::from_secs(25), client.closed.cancelled())
        .await
        .unwrap();
    assert!(!client.rejected.load(Ordering::SeqCst));
    assert_eq!(
        fixture
            .client
            .lock()
            .unwrap()
            .client(now())
            .unwrap()
            .chain
            .room(),
        chain
    );
    assert!(client.rx.try_recv().is_err());
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_session_does_not_trigger_an_automatic_new_handshake_or_reset_credentials() {
    let fixture = Fixture::with_fault(7).await;
    let (mut client, mut host) = fixture.pair().await;
    let chain = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    let hellos = fixture.fault.hellos.load(Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(3), client.closed.cancelled())
        .await
        .unwrap();
    assert!(!client.rejected.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fixture.fault.hellos.load(Ordering::SeqCst), hellos);
    assert_eq!(
        fixture
            .client
            .lock()
            .unwrap()
            .client(now())
            .unwrap()
            .chain
            .room(),
        chain
    );
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_handshake_reply_cannot_advance_a_new_client_attempt() {
    let fixture = Fixture::with_fault(8).await;
    let (mut client, mut host) = fixture.pair().await;
    client.close().await;
    host.close().await;
    let chain = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    let stop = CancellationToken::new();
    let options = fixture.options.clone();
    let host_store = fixture.host.clone();
    let stopping = stop.clone();
    let accepting = tokio::spawn(async move { accept(&options, host_store, stopping).await });
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        connect(&fixture.options, fixture.client.clone(), stop.clone()),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert_eq!(
        fixture
            .client
            .lock()
            .unwrap()
            .client(now())
            .unwrap()
            .chain
            .room(),
        chain
    );
    stop.cancel();
    assert!(accepting.await.unwrap().is_err());
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_authenticated_connect_proxy_carries_tls_without_disabling_certificate_checks() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    let target = url::Url::parse(&fixture.options.relays[0])
        .unwrap()
        .port()
        .unwrap();
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = connections.clone();
    let stop = CancellationToken::new();
    let stopping = stop.clone();
    let serving = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            let accepted =
                tokio::select! {_=stopping.cancelled()=>break,item=listener.accept()=>item};
            let (mut stream, _) = accepted.unwrap();
            let observed = observed.clone();
            tasks.spawn(async move {
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).await.unwrap();
                    header.push(byte[0]);
                    assert!(header.len() < 8192);
                }
                let header = String::from_utf8(header).unwrap();
                assert!(header.starts_with(&format!("CONNECT localhost:{target} HTTP/1.1\r\n")));
                assert!(header.lines().any(|line| {
                    line.eq_ignore_ascii_case("proxy-authorization: Basic cHJveHk6cGFzc3dvcmQ=")
                }));
                let mut remote = tokio::net::TcpStream::connect(("127.0.0.1", target))
                    .await
                    .unwrap();
                stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await
                    .unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut remote).await;
            });
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    });
    let mut options = fixture.options.clone();
    options.proxy_url = Some(format!("http://proxy:password@{proxy}"));
    let pair_stop = CancellationToken::new();
    let (client, host) = tokio::join!(
        connect(&options, fixture.client.clone(), pair_stop.clone()),
        accept(&fixture.options, fixture.host.clone(), pair_stop)
    );
    let mut client = client.unwrap();
    let mut host = host.unwrap();
    client.tx.send(Frame::Credit { id: 12 }).await.unwrap();
    assert!(matches!(receive(&mut host).await, Frame::Credit { id: 12 }));
    assert!(connections.load(Ordering::SeqCst) > 0);
    client.close().await;
    host.close().await;
    stop.cancel();
    serving.await.unwrap();
    fixture.finish().await;
}
async fn receive(tunnel: &mut HttpsTunnel) -> Frame {
    tokio::time::timeout(Duration::from_secs(5), tunnel.rx.recv())
        .await
        .unwrap()
        .expect("verified application frame expected")
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_tls_negotiation_carries_frames_in_both_directions_and_advances_saved_credentials() {
    let fixture = Fixture::new().await;
    let before = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    let (mut client, mut host) = fixture.pair().await;
    let after = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    assert_ne!(before, after);
    assert_eq!(
        fixture
            .host
            .lock()
            .unwrap()
            .host(now())
            .unwrap()
            .chains
            .current()
            .room(),
        after
    );
    client
        .tx
        .send(Frame::Request {
            id: 1,
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut host).await,
        Frame::Request { id: 1, .. }
    ));
    for payload in [
        b"data: thinking\n\n".as_slice(),
        b"data: tool arguments\n\n",
        b"data: answer\n\n",
    ] {
        host.tx.send(Frame::data(1, payload)).await.unwrap();
        let Frame::Data { id, data } = receive(&mut client).await else {
            panic!()
        };
        assert_eq!(id, 1);
        assert_eq!(decode_data(&data).unwrap(), payload);
    }
    host.tx.send(Frame::End { id: 1 }).await.unwrap();
    assert!(matches!(receive(&mut client).await, Frame::End { id: 1 }));
    client.close().await;
    host.close().await;
    assert!(!client.rejected.load(Ordering::SeqCst));
    assert!(!host.rejected.load(Ordering::SeqCst));
    let (mut client, mut host) = fixture.pair().await;
    let next = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    assert_ne!(after, next);
    assert_eq!(
        fixture
            .host
            .lock()
            .unwrap()
            .host(now())
            .unwrap()
            .chains
            .current()
            .room(),
        next
    );
    client.tx.send(Frame::Credit { id: 2 }).await.unwrap();
    assert!(matches!(receive(&mut host).await, Frame::Credit { id: 2 }));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn untrusted_tls_and_invalid_origins_never_advance_the_chain() {
    let fixture = Fixture::new().await;
    let chain = fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room();
    let mut options = fixture.options.clone();
    options.ca_cert_file = None;
    let http = Http::new(&options).unwrap();
    let result = http
        .get_bytes(
            "/v1/hello/invalid/client?wait=0",
            0,
            HELLO_BYTES,
            &CancellationToken::new(),
            Instant::now() + Duration::from_millis(250),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        fixture
            .client
            .lock()
            .unwrap()
            .client(now())
            .unwrap()
            .chain
            .room(),
        chain
    );
    let mut options = fixture.options.clone();
    options.relays.push(options.relays[0].clone());
    assert!(Http::new(&options).is_err());
    options.relays = vec!["http://localhost".into()];
    assert!(Http::new(&options).is_err());
    fixture.finish().await;
}
#[tokio::test]
async fn relay_redirects_are_denied_instead_of_followed() {
    let fixture = Fixture::with_fault(9).await;
    let http = Http::new(&fixture.options).unwrap();
    let error = http
        .get(
            "/unused",
            0,
            &CancellationToken::new(),
            Instant::now() + Duration::from_secs(2),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.downcast_ref::<Failure>(), Some(&Failure::Denied));
    fixture.finish().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn encrypted_idle_heartbeats_refresh_peer_liveness_without_becoming_application_frames() {
    let fixture = Fixture::new().await;
    let (mut client, mut host) = fixture.pair().await;
    tokio::time::sleep(Duration::from_secs(11)).await;
    assert!(!client.closed.is_cancelled() && !host.closed.is_cancelled());
    assert!(
        client
            .core
            .as_ref()
            .unwrap()
            .cipher
            .lock()
            .unwrap()
            .receive_sequence()
            >= 2
    );
    assert!(
        host.core
            .as_ref()
            .unwrap()
            .cipher
            .lock()
            .unwrap()
            .receive_sequence()
            >= 2
    );
    assert!(matches!(
        client.rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        host.rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[derive(Clone)]
struct Backend {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    cancelled: Arc<std::sync::atomic::AtomicUsize>,
    release: Arc<tokio::sync::Semaphore>,
}
struct CancelGuard(Backend, bool);
impl Drop for CancelGuard {
    fn drop(&mut self) {
        if self.1 {
            self.0.cancelled.fetch_add(1, Ordering::SeqCst);
        }
    }
}
async fn mock_completion(
    axum::extract::State(backend): axum::extract::State<Backend>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::response::Response {
    backend.calls.fetch_add(1, Ordering::SeqCst);
    if body["model"] == "count" {
        let stream = futures_util::stream::iter(
            counted()
                .into_iter()
                .map(|line| Ok::<_, std::convert::Infallible>(Bytes::from(line))),
        )
        .then(|line| async {
            tokio::time::sleep(Duration::from_micros(200)).await;
            line
        });
        return axum::response::Response::builder()
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap();
    }
    let cancel = body["model"] == "cancel";
    let guard = CancelGuard(backend.clone(), cancel);
    let stream = async_stream::stream! {
        let _guard=guard;
        yield Ok::<_,std::convert::Infallible>(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"checking\"}}]}\n\n"));
        if cancel {
            loop { tokio::time::sleep(Duration::from_millis(50)).await;yield Ok(Bytes::from_static(b": keepalive\n\n")); }
        }
        let permit=backend.release.acquire().await.unwrap();permit.forget();
        yield Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\"}}]}}]}\n\n"));
        yield Ok(Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n"));
    };
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wired_host_and_gateway_stream_thinking_tools_and_parallel_requests_and_cancel_without_replay()
 {
    wired(6).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buffering_finite_put_and_get_bodies_preserves_live_thinking_tools_and_cancellation() {
    wired(10).await;
}
struct Wired {
    gateway: crate::gateway::Gateway,
    api: String,
    api_key: String,
    http: reqwest::Client,
    backend: Backend,
    stop: CancellationToken,
    tasks: [JoinHandle<()>; 4],
}
/// The actual gateway, host and a mock backend joined through the fixture's relay.
/// The Anthropic Messages events of one short streamed answer.
const MESSAGE_EVENTS: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"local\",\"content\":[]}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);
async fn mock_messages(
    axum::extract::State(backend): axum::extract::State<Backend>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    backend.calls.fetch_add(1, Ordering::SeqCst);
    // The host pins the version; the caller's beta list and credential stay local.
    assert_eq!(
        headers["anthropic-version"],
        k_lani_link_protocol::ANTHROPIC_VERSION
    );
    assert!(!headers.contains_key("anthropic-beta") && !headers.contains_key("x-api-key"));
    axum::response::Response::builder()
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from(MESSAGE_EVENTS))
        .unwrap()
}
/// About 300 KB of small SSE events: several response windows.
fn counted() -> Vec<String> {
    (0..4000)
        .map(|n| format!("data: {{\"n\":{n},\"pad\":\"{}\"}}\n\n", "x".repeat(40)))
        .chain(["data: [DONE]\n\n".to_owned()])
        .collect()
}
async fn wired_lab(fixture: &Fixture) -> Wired {
    wired_resume(fixture, 0, 0).await
}
async fn wired_resume(fixture: &Fixture, host_resume: u64, client_resume: u64) -> Wired {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    let root = &fixture._cleanup.0;
    let chain = Chain::from_code(&random_secret().unwrap()).unwrap();
    for (folder, role) in [
        ("wired-host", EndpointRole::Host),
        ("wired-client", EndpointRole::Client),
    ] {
        let mut store = Store::open(&root.join(folder), role).unwrap();
        store
            .initialize(&chain, now(), Lifetimes::default())
            .unwrap();
    }
    let backend = Backend {
        calls: Arc::default(),
        cancelled: Arc::default(),
        release: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let router=Router::new().route("/health",get(||async { Json(serde_json::json!({"link_privacy":{"version":1,"application_logs":false,"request_history":false,"diagnostics":false,"remote_images":false}})) }))
        .route("/v1/models",get(||async { Json(serde_json::json!({"data":[{"id":"https-test"}]})) }))
        .route("/v1/chat/completions",post(mock_completion))
        .route("/v1/messages",post(mock_messages)).with_state(backend.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let stopping = stop.clone();
    let engine = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopping.cancelled_owned())
            .await
            .unwrap();
    });
    let api_key = random_secret().unwrap();
    let mut host_options = fixture.options.clone();
    host_options.state_dir = root.join("wired-host");
    let mut client_options = fixture.options.clone();
    client_options.state_dir = root.join("wired-client");
    let mut host_config = super::super::setup::config(host_options, api_key.clone());
    host_config.resume_seconds = host_resume;
    host_config.upstream = upstream;
    host_config.max_concurrent_requests = 2;
    let mut client_config = super::super::setup::config(client_options, api_key.clone());
    client_config.resume_seconds = client_resume;
    client_config.max_concurrent_requests = 2;
    let gateway = crate::gateway::Gateway::new(client_config).unwrap();
    let serving = gateway.clone();
    let stopping = stop.clone();
    let unused = root.join("unused-legacy-client-state");
    let client = tokio::spawn(async move {
        serving.maintain(stopping, unused).await;
    });
    let stopping = stop.clone();
    let unused = root.join("unused-legacy-host-state");
    let host = tokio::spawn(async move {
        crate::host::run(host_config, stopping, unused)
            .await
            .unwrap();
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", listener.local_addr().unwrap());
    let router = gateway.router();
    let stopping = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopping.cancelled_owned())
            .await
            .unwrap();
    });
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    Wired {
        gateway,
        api,
        api_key,
        http,
        backend,
        stop,
        tasks: [client, host, server, engine],
    }
}
async fn wired(mode: u8) {
    let fixture = Fixture::with_fault(mode).await;
    let lab = wired_lab(&fixture).await;
    connected(&lab, 5).await;
    let root = &fixture._cleanup.0;
    assert!(
        !root.join("unused-legacy-client-state").exists()
            && !root.join("unused-legacy-host-state").exists()
    );
    let Wired {
        gateway: _,
        api,
        api_key,
        http,
        backend,
        stop,
        tasks,
    } = lab;
    let state: serde_json::Value = http
        .get(format!("{api}/health"))
        .bearer_auth(&api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(state["transport"], "https");
    assert_eq!(state["data_relay"], true);
    assert_eq!(
        http.get(format!("{api}/v1/models"))
            .bearer_auth(&api_key)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let request = || {
        http.post(format!("{api}/v1/chat/completions")).bearer_auth(&api_key)
        .json(&serde_json::json!({"model":"stream","stream":true,"store":false,"messages":[{"role":"user","content":"synthetic test"}]})).send()
    };
    let (one, two) = tokio::join!(request(), request());
    let mut one = one.unwrap().bytes_stream();
    let mut two = two.unwrap().bytes_stream();
    let first = one.next().await.unwrap().unwrap();
    let second = two.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&first).contains("reasoning_content"));
    assert!(String::from_utf8_lossy(&second).contains("reasoning_content"));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    backend.release.add_permits(2);
    for stream in [&mut one, &mut two] {
        let mut output = Vec::new();
        while let Some(chunk) = stream.next().await {
            output.extend_from_slice(&chunk.unwrap());
        }
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.contains("tool_calls") && output.contains("hello") && output.contains("[DONE]")
        );
    }
    let cancelled = http
        .post(format!("{api}/v1/chat/completions"))
        .bearer_auth(&api_key)
        .json(&serde_json::json!({"model":"cancel","stream":true,"store":false,"messages":[]}))
        .send()
        .await
        .unwrap();
    let mut cancelled = cancelled.bytes_stream();
    cancelled.next().await.unwrap().unwrap();
    drop(cancelled);
    tokio::time::timeout(Duration::from_secs(3), async {
        while backend.cancelled.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(backend.calls.load(Ordering::SeqCst), 3);
    let hashes = fixture.fault.put_hashes.lock().unwrap().clone();
    if mode == 6 {
        assert!(hashes.len() >= 2 && hashes.iter().all(|h| h == &hashes[0]));
    }
    stop.cancel();
    for task in tasks {
        task.await.unwrap();
    }
    fixture.finish().await;
}

fn failure<T>(result: Result<T>) -> anyhow::Error {
    match result {
        Ok(_) => panic!("expected the handshake to fail"),
        Err(error) => error,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_transport_failure_after_delivery_marks_a_hello_as_possibly_pending() {
    // No host runs. An absent host leaves every poll unanswered (204 or the deadline cut);
    // a failing poll could have hidden a reply the host saved as pending.
    let (absent, failing, unacknowledged) = tokio::join!(
        Fixture::new(),
        Fixture::with_fault(11),
        Fixture::with_fault(14)
    );
    let stop = CancellationToken::new();
    let (quiet, lost, unconfirmed) = tokio::join!(
        connect(&absent.options, absent.client.clone(), stop.clone()),
        connect(&failing.options, failing.client.clone(), stop.clone()),
        connect(
            &unacknowledged.options,
            unacknowledged.client.clone(),
            stop.clone()
        ),
    );
    let (quiet, lost, unconfirmed) = (failure(quiet), failure(lost), failure(unconfirmed));
    // Not one PUT was confirmed, yet the relay holds the hello.
    assert!(unconfirmed.is::<UnansweredHello>());
    // Still a transient network failure for the gateway, never a pairing rejection.
    assert_eq!(
        unconfirmed.downcast_ref::<Failure>(),
        Some(&Failure::Network)
    );
    assert!(unacknowledged.fault.hellos.load(Ordering::SeqCst) >= 1);
    assert_eq!(quiet.downcast_ref::<Failure>(), Some(&Failure::Network));
    assert!(!quiet.is::<UnansweredHello>());
    assert_eq!(lost.downcast_ref::<Failure>(), Some(&Failure::Network));
    assert!(lost.is::<UnansweredHello>());
    assert!(absent.fault.hellos.load(Ordering::SeqCst) >= 1);
    assert!(failing.fault.hellos.load(Ordering::SeqCst) >= 1);
    absent.finish().await;
    failing.finish().await;
    unacknowledged.finish().await;
}

async fn health(lab: &Wired) -> serde_json::Value {
    lab.http
        .get(format!("{}/health", lab.api))
        .bearer_auth(&lab.api_key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn connected(lab: &Wired, seconds: u64) {
    tokio::time::timeout(Duration::from_secs(seconds), async {
        while !lab.gateway.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_gateway_negotiates_a_fresh_session_after_a_relay_restart_without_replaying_work() {
    let mut fixture = Fixture::new().await;
    let lab = wired_lab(&fixture).await;
    connected(&lab, 5).await;
    let generation = lab
        .http
        .post(format!("{}/v1/chat/completions", lab.api))
        .bearer_auth(&lab.api_key)
        .json(&serde_json::json!({"model":"cancel","stream":true,"store":false,"messages":[]}))
        .send()
        .await
        .unwrap();
    let mut generation = generation.bytes_stream();
    generation.next().await.unwrap().unwrap();
    let hellos = fixture.fault.hellos.load(Ordering::SeqCst);
    fixture.restart_relay().await;
    let rest = tokio::time::timeout(Duration::from_secs(10), async {
        let mut rest = Vec::new();
        while let Some(Ok(chunk)) = generation.next().await {
            rest.extend_from_slice(&chunk);
        }
        rest
    })
    .await
    .unwrap();
    assert!(!String::from_utf8_lossy(&rest).contains("[DONE]"));
    // The host dropped the backend request: the model stops instead of generating unread.
    tokio::time::timeout(Duration::from_secs(10), async {
        while lab.backend.cancelled.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    connected(&lab, 15).await;
    assert!(fixture.fault.hellos.load(Ordering::SeqCst) > hellos);
    let state = health(&lab).await;
    assert_eq!(state["connected"], true);
    assert_eq!(state["reconnecting"], false);
    assert_eq!(state["reconnect_required"], false);
    assert_eq!(state["pairing_rejected"], false);
    assert_eq!(
        lab.http
            .get(format!("{}/v1/models", lab.api))
            .bearer_auth(&lab.api_key)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    assert_eq!(lab.backend.calls.load(Ordering::SeqCst), 1);
    lab.stop.cancel();
    for task in lab.tasks {
        task.await.unwrap();
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_sessions_stop_before_lost_replies_can_exhaust_the_pending_attempts() {
    let fixture = Fixture::with_fault(11).await;
    let lab = wired_lab(&fixture).await;
    tokio::time::timeout(Duration::from_secs(60), async {
        while health(&lab).await["reconnect_required"] != true {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let state = health(&lab).await;
    assert_eq!(state["pairing_rejected"], false);
    assert_eq!(state["reconnecting"], false);
    let hellos = fixture.fault.hellos.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(fixture.fault.hellos.load(Ordering::SeqCst), hellos);
    // One pending attempt is left: an explicit restart still pairs without a new code.
    fixture.fault.enabled.store(false, Ordering::SeqCst);
    let restart = lab.gateway.clone();
    let stopping = lab.stop.clone();
    let unused = fixture._cleanup.0.join("unused-legacy-client-state");
    let again = tokio::spawn(async move { restart.maintain(stopping, unused).await });
    connected(&lab, 30).await;
    lab.stop.cancel();
    again.await.unwrap();
    for task in lab.tasks {
        task.await.unwrap();
    }
    fixture.finish().await;
}

async fn count(lab: &Wired) -> impl futures_util::Stream<Item = reqwest::Result<Bytes>> {
    lab.http
        .post(format!("{}/v1/chat/completions", lab.api))
        .bearer_auth(&lab.api_key)
        .json(&serde_json::json!({"model":"count","stream":true,"store":false,"messages":[]}))
        .send()
        .await
        .unwrap()
        .bytes_stream()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_resume_enabled_a_response_continues_exactly_once_after_a_relay_restart() {
    let mut fixture = Fixture::new().await;
    let lab = wired_resume(&fixture, 30, 30).await;
    connected(&lab, 5).await;
    let hellos = fixture.fault.hellos.load(Ordering::SeqCst);
    let mut stream = count(&lab).await;
    let mut output = stream.next().await.unwrap().unwrap().to_vec();
    fixture.restart_relay().await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(chunk) = stream.next().await {
            output.extend_from_slice(&chunk.unwrap());
        }
    })
    .await
    .unwrap();
    // No gap and no duplicate: the bytes equal one uninterrupted run.
    assert_eq!(String::from_utf8(output).unwrap(), counted().concat());
    assert_eq!(lab.backend.calls.load(Ordering::SeqCst), 1);
    // A new session carried the rest, not the old one.
    assert!(fixture.fault.hellos.load(Ordering::SeqCst) > hellos);
    drop(stream);
    lab.stop.cancel();
    for task in lab.tasks {
        task.await.unwrap();
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_detached_response_stops_the_backend_when_nobody_resumes_it_in_time() {
    let mut fixture = Fixture::new().await;
    let lab = wired_resume(&fixture, 2, 2).await;
    connected(&lab, 5).await;
    let generation = lab
        .http
        .post(format!("{}/v1/chat/completions", lab.api))
        .bearer_auth(&lab.api_key)
        .json(&serde_json::json!({"model":"cancel","stream":true,"store":false,"messages":[]}))
        .send()
        .await
        .unwrap();
    let mut generation = generation.bytes_stream();
    generation.next().await.unwrap().unwrap();
    // The client disappears for good; the host only learns that the session ended.
    let [client, host, server, engine] = lab.tasks;
    client.abort();
    fixture.restart_relay().await;
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(20), async {
        while lab.backend.cancelled.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2));
    // The caller's local response ends with an error instead of hanging.
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(Ok(_)) = generation.next().await {}
    })
    .await
    .unwrap();
    lab.stop.cancel();
    let _ = client.await;
    for task in [host, server, engine] {
        task.await.unwrap();
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_without_resume_answers_a_resume_with_a_clean_error() {
    let mut fixture = Fixture::new().await;
    let lab = wired_resume(&fixture, 0, 30).await;
    connected(&lab, 5).await;
    let mut stream = count(&lab).await;
    let mut output = stream.next().await.unwrap().unwrap().to_vec();
    fixture.restart_relay().await;
    let failed = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => output.extend_from_slice(&chunk),
                Err(_) => return true,
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(failed);
    assert!(!String::from_utf8_lossy(&output).contains("[DONE]"));
    assert_eq!(lab.backend.calls.load(Ordering::SeqCst), 1);
    drop(stream);
    lab.stop.cancel();
    for task in lab.tasks {
        task.await.unwrap();
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claude_code_messages_request_streams_through_the_tunnel() {
    let fixture = Fixture::new().await;
    let lab = wired_lab(&fixture).await;
    connected(&lab, 5).await;
    // The request as captured from Claude Code 2.1.295 with ANTHROPIC_API_KEY.
    let response = lab
        .http
        .post(format!("{}/v1/messages?beta=true", lab.api))
        .header("x-api-key", &lab.api_key)
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "claude-code-20250219,interleaved-thinking-2025-05-14")
        .header("anthropic-dangerous-direct-browser-access", "true")
        .json(&serde_json::json!({"model":"local","max_tokens":32000,"stream":true,"messages":[{"role":"user","content":"hi"}]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(response.text().await.unwrap(), MESSAGE_EVENTS);
    assert_eq!(lab.backend.calls.load(Ordering::SeqCst), 1);
    lab.stop.cancel();
    for task in lab.tasks {
        task.await.unwrap();
    }
    fixture.finish().await;
}

async fn relay_version(fixture: &Fixture, http1_only: bool) -> reqwest::Version {
    let pem = std::fs::read(fixture.options.ca_cert_file.as_ref().unwrap()).unwrap();
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .add_root_certificate(reqwest::Certificate::from_pem(&pem).unwrap());
    if http1_only {
        client = client.http1_only();
    }
    client
        .build()
        .unwrap()
        .get(&fixture.options.relays[0])
        .send()
        .await
        .unwrap()
        .version()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_relay_offers_http2_and_falls_back_to_http1_with_the_same_session_semantics() {
    let mut fixture = Fixture::new().await;
    assert_eq!(
        relay_version(&fixture, false).await,
        reqwest::Version::HTTP_2
    );
    assert_eq!(
        relay_version(&fixture, true).await,
        reqwest::Version::HTTP_11
    );
    let (mut client, mut host) = fixture.pair().await;
    client.tx.send(Frame::Credit { id: 5 }).await.unwrap();
    assert!(matches!(receive(&mut host).await, Frame::Credit { id: 5 }));
    client.close().await;
    host.close().await;
    // A relay (or TLS-inspecting proxy) without HTTP/2: the same clients fall back.
    fixture.relay_http1 = true;
    fixture.restart_relay().await;
    assert_eq!(
        relay_version(&fixture, false).await,
        reqwest::Version::HTTP_11
    );
    let (mut client, mut host) = fixture.pair().await;
    host.tx.send(Frame::Credit { id: 6 }).await.unwrap();
    assert!(matches!(
        receive(&mut client).await,
        Frame::Credit { id: 6 }
    ));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

fn client_room(fixture: &Fixture) -> String {
    fixture
        .client
        .lock()
        .unwrap()
        .client(now())
        .unwrap()
        .chain
        .room()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http3_carries_a_session_and_a_lost_quic_path_falls_back_to_tcp_in_the_same_session() {
    let mut fixture = Fixture::new().await;
    fixture.options.http3 = true;
    let quic = fixture.start_quic();
    let (mut client, mut host) = fixture.pair().await;
    client.tx.send(Frame::Credit { id: 1 }).await.unwrap();
    assert!(matches!(receive(&mut host).await, Frame::Credit { id: 1 }));
    assert!(fixture.fault.quic_requests.load(Ordering::SeqCst) > 0);
    let (hellos, room) = (
        fixture.fault.hellos.load(Ordering::SeqCst),
        client_room(&fixture),
    );
    // UDP becomes unusable mid-session: same keys, cursors and session over TCP.
    quic.cancel();
    for id in 2..6 {
        client.tx.send(Frame::Credit { id }).await.unwrap();
        assert!(matches!(receive(&mut host).await, Frame::Credit { id: got } if got == id));
        host.tx.send(Frame::Credit { id }).await.unwrap();
        assert!(matches!(receive(&mut client).await, Frame::Credit { id: got } if got == id));
    }
    assert_eq!(fixture.fault.hellos.load(Ordering::SeqCst), hellos);
    assert_eq!(client_room(&fixture), room);
    assert!(!client.closed.is_cancelled() && !host.closed.is_cancelled());
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn http3_without_reachable_udp_pairs_over_tcp_after_one_short_attempt() {
    let mut fixture = Fixture::new().await;
    fixture.options.http3 = true;
    let started = std::time::Instant::now();
    let (mut client, mut host) = tokio::time::timeout(Duration::from_secs(15), fixture.pair())
        .await
        .unwrap();
    client.tx.send(Frame::Credit { id: 9 }).await.unwrap();
    assert!(matches!(receive(&mut host).await, Frame::Credit { id: 9 }));
    assert_eq!(fixture.fault.quic_requests.load(Ordering::SeqCst), 0);
    assert!(started.elapsed() < Duration::from_secs(10));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[test]
fn http3_cannot_be_combined_with_an_explicit_proxy() {
    let root = std::env::temp_dir();
    let options = super::super::Options {
        relays: vec!["https://relay.example:443".into()],
        state_dir: root.clone(),
        admission_token_file: root,
        proxy_url: Some("http://proxy.example:3128".into()),
        ca_cert_file: None,
        poll_wait_seconds: 25,
        http1_only: false,
        http3: true,
        post_writes: false,
    };
    assert!(options.validate().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_that_refuses_put_carries_the_whole_session_over_post() {
    let fixture = Fixture::with_fault(12).await;
    let (mut client, mut host) = fixture.pair().await;
    for id in 1..4 {
        client.tx.send(Frame::Credit { id }).await.unwrap();
        assert!(matches!(receive(&mut host).await, Frame::Credit { id: got } if got == id));
        host.tx.send(Frame::Credit { id }).await.unwrap();
        assert!(matches!(receive(&mut client).await, Frame::Credit { id: got } if got == id));
    }
    assert!(!client.rejected.load(Ordering::SeqCst) && !host.rejected.load(Ordering::SeqCst));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[test]
fn a_relay_may_be_mounted_below_a_plain_path() {
    let root = std::env::temp_dir();
    let options = |relay: &str| super::super::Options {
        relays: vec![relay.into()],
        state_dir: root.clone(),
        admission_token_file: root.clone(),
        proxy_url: None,
        ca_cert_file: None,
        poll_wait_seconds: 25,
        http1_only: false,
        http3: false,
        post_writes: false,
    };
    for (relay, valid) in [
        ("https://relay.example", true),
        ("https://kunde.example/wp-json/k-lani-link", true),
        ("https://kunde.example/relay/", true),
        // URL parsing already resolves dot segments, so `/a/../b` arrives as `/b`.
        ("https://kunde.example/a/../b", true),
        ("https://kunde.example/a//b", false),
        ("https://kunde.example/a%2Fb", false),
        ("https://kunde.example/a?x=1", false),
        ("https://user:pw@kunde.example/relay", false),
        ("http://kunde.example/relay", false),
    ] {
        assert_eq!(options(relay).validate().is_ok(), valid, "{relay}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_firewall_admitting_only_json_carries_the_session_as_wrapped_records() {
    let fixture = Fixture::with_fault(13).await;
    let (mut client, mut host) = fixture.pair().await;
    for id in 1..4 {
        client.tx.send(Frame::Credit { id }).await.unwrap();
        assert!(matches!(receive(&mut host).await, Frame::Credit { id: got } if got == id));
        host.tx.send(Frame::Credit { id }).await.unwrap();
        assert!(matches!(receive(&mut client).await, Frame::Credit { id: got } if got == id));
    }
    assert!(!client.rejected.load(Ordering::SeqCst) && !host.rejected.load(Ordering::SeqCst));
    client.close().await;
    host.close().await;
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_http3_upload_without_a_token_is_refused_before_it_is_read() {
    let fixture = Fixture::new().await;
    let quic = fixture.start_quic();
    let pem = std::fs::read(fixture.options.ca_cert_file.as_ref().unwrap()).unwrap();
    let client = super::super::quic::Client::new(&fixture.options.relays[0], Some(&pem)).unwrap();
    let mut sender = client.sender().await.unwrap();
    let port = url::Url::parse(&fixture.options.relays[0])
        .unwrap()
        .port()
        .unwrap();
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://localhost:{port}/v1/box/a/b/c2h/1"))
        .body(())
        .unwrap();
    let mut stream = sender.send_request(request).await.unwrap();
    stream
        .send_data(Bytes::from(vec![0u8; 1024]))
        .await
        .unwrap();
    // The upload is not finished; the refusal must not wait for it.
    let response = tokio::time::timeout(Duration::from_secs(5), stream.recv_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 401);
    quic.cancel();
    fixture.finish().await;
}
