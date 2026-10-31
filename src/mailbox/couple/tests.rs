use super::*;
use crate::config::{Config, private_file, random_id, random_secret};
use crate::mailbox::relay::{Access, Admit, Brake, Limits, Relay};
use tokio_util::sync::CancellationToken;

pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) relay: String,
    host_token: PathBuf,
    client_token: String,
    pub(crate) ca: PathBuf,
    stop: CancellationToken,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
async fn tls() -> (axum_server::tls_rustls::RustlsConfig, String) {
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        certificate.cert.pem().into_bytes(),
        certificate.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    (tls, certificate.cert.pem())
}
async fn serve(router: axum::Router, root: &Path) -> (String, PathBuf, CancellationToken) {
    let (tls, pem) = tls().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    tokio::spawn(crate::mailbox::server::serve_router(
        listener,
        router,
        Some(tls),
        64,
        false,
        stop.clone(),
    ));
    let ca = root.join("ca.pem");
    private_file(&ca, &pem).unwrap();
    (format!("https://localhost:{port}"), ca, stop)
}
pub(crate) async fn fixture() -> Fixture {
    fixture_with(Limits::default()).await
}
async fn fixture_with(global: Limits) -> Fixture {
    let root = std::env::temp_dir().join(format!("link-couple-{}", random_id().unwrap()));
    crate::pairing::private_directory(&root).unwrap();
    let (host, client) = (random_secret().unwrap(), random_secret().unwrap());
    let admit = |key: &str| Admit {
        key: key.into(),
        space: Some("pair".into()),
        limits: None,
        access: Access::default(),
    };
    let relay =
        Relay::with_access(global, vec![admit(&host), admit(&client)], Brake::default()).unwrap();
    let stop = CancellationToken::new();
    let (url, ca, serving) = serve(relay.router(stop.clone()), &root).await;
    let child = stop.clone();
    tokio::spawn(async move {
        child.cancelled().await;
        serving.cancel();
    });
    let host_token = root.join("host-token");
    private_file(&host_token, &host).unwrap();
    Fixture {
        root,
        relay: url,
        host_token,
        client_token: client,
        ca,
        stop,
    }
}
impl Fixture {
    fn options(&self) -> Options {
        Options {
            relays: vec![self.relay.clone()],
            state_dir: self.root.join("unused"),
            admission_token_file: self.host_token.clone(),
            proxy_url: None,
            ca_cert_file: Some(self.ca.clone()),
            poll_wait_seconds: 25,
            http1_only: false,
            http3: false,
            post_writes: false,
        }
    }
    fn local(&self) -> Join<'_> {
        Join {
            relay: &self.relay,
            proxy_url: None,
            ca_cert_file: Some(self.ca.clone()),
        }
    }
    /// Starts the host side and returns its task and the code it showed.
    pub(crate) async fn offer(&self) -> (tokio::task::JoinHandle<Result<()>>, String) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (dir, options, token) = (
            self.root.join("pairing"),
            self.options(),
            self.client_token.clone(),
        );
        let task = tokio::spawn(async move {
            offer(
                &dir,
                options,
                &token,
                Lifetimes::default(),
                &Default::default(),
                |code| {
                    let _ = sender.send(code.to_owned());
                },
            )
            .await
        });
        (task, receiver.await.unwrap())
    }
}

#[test]
fn codes_round_trip_and_reject_anything_but_eleven_digits() {
    let code = Code::new(472).unwrap();
    let text = code.to_string();
    assert!(text.starts_with("472-") && text.len() == 13);
    let parsed = Code::parse(&text.replace('-', " ")).unwrap();
    assert_eq!((parsed.slot, &parsed.password), (472, &code.password));
    // As a terminal or a Windows pipe hands it over.
    let typed = Code::parse(&format!(" {text}\r\n")).unwrap();
    assert_eq!((typed.slot, typed.password), (472, code.password));
    for bad in [
        "",
        "472-1234-567",
        "472-1234-56789",
        "072-1234-5678",
        "47a-1234-5678",
    ] {
        assert!(Code::parse(bad).is_err(), "{bad}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_code_gives_the_client_the_same_pairing_with_its_own_token() {
    let fixture = fixture().await;
    let (host, code) = fixture.offer().await;
    // Typed differently from the host's address: same relay, same keys.
    let typed = format!("{}/", fixture.relay.replace("localhost", "LOCALHOST"));
    let client = join(
        &fixture.root.join("laptop"),
        &Code::parse(&code).unwrap(),
        Join {
            relay: &typed,
            proxy_url: None,
            ca_cert_file: Some(fixture.ca.clone()),
        },
    )
    .await
    .unwrap();
    host.await.unwrap().unwrap();
    // The host closed the slot after checking the acknowledgement.
    let ca = std::fs::read(&fixture.ca).unwrap();
    let probe = Slots::new(&fixture.relay, None, Some(&ca), None).unwrap();
    let slot = Code::parse(&code).unwrap().slot;
    assert!(matches!(
        probe.read(slot, "c", 0).await.unwrap(),
        Read::Missing
    ));
    let host_config = Config::load(&fixture.root.join("pairing/host/host.toml")).unwrap();
    let client_config = Config::load(&client).unwrap();
    assert_eq!(host_config.api_key, client_config.api_key);
    let options = client_config.https.as_ref().unwrap();
    assert_eq!(options.relays, vec![fixture.relay.clone()]);
    assert_eq!(
        super::super::secret(&options.admission_token_file).unwrap(),
        fixture.client_token
    );
    // The client trusts the CA it brought itself; the bundle carries none.
    assert!(fixture.root.join("laptop/ca.pem").exists());
    // A real encrypted session between the two folders proves they share one pairing.
    let open = |config: &Config, role| {
        let options = config.https.clone().unwrap();
        let store = crate::chain_state::Store::open(&options.state_dir, role).unwrap();
        (options, std::sync::Arc::new(std::sync::Mutex::new(store)))
    };
    let (client_options, client_store) = open(&client_config, EndpointRole::Client);
    let (host_options, host_store) = open(&host_config, EndpointRole::Host);
    let stop = CancellationToken::new();
    let (mut laptop, mut server) = tokio::join!(
        crate::mailbox::transport::connect(&client_options, client_store, stop.clone()),
        crate::mailbox::transport::accept(&host_options, host_store, stop.clone()),
    );
    let (laptop, server) = (laptop.as_mut().unwrap(), server.as_mut().unwrap());
    laptop
        .tx
        .send(k_lani_link_protocol::Frame::Credit { id: 7 })
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(10), server.rx.recv())
        .await
        .unwrap();
    assert!(matches!(
        received,
        Some(k_lani_link_protocol::Frame::Credit { id: 7 })
    ));
    stop.cancel();
    // A used code cannot pair a second computer.
    let again = join(
        &fixture.root.join("second"),
        &Code::parse(&code).unwrap(),
        fixture.local(),
    )
    .await;
    assert!(again.is_err());
    assert!(!fixture.root.join("second").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wrong_code_costs_the_slot_and_leaves_no_folder_behind() {
    let fixture = fixture().await;
    let (host, code) = fixture.offer().await;
    let mut wrong = Code::parse(&code).unwrap();
    wrong.password = if wrong.password == "00000000" {
        "11111111".into()
    } else {
        "00000000".into()
    };
    // Bounded: if the host forgot to close the slot, the client would poll for ten minutes.
    let error = tokio::time::timeout(
        Duration::from_secs(30),
        join(&fixture.root.join("laptop"), &wrong, fixture.local()),
    )
    .await
    .expect("a refused code must end the client's wait at once")
    .unwrap_err();
    assert!(error.to_string().contains("not accepted"), "{error}");
    let refused = host.await.unwrap().unwrap_err();
    assert!(refused.to_string().contains("entered wrongly"), "{refused}");
    assert!(!fixture.root.join("pairing").exists());
    assert!(!fixture.root.join("laptop").exists());
    // The right code is useless now: the slot is gone.
    let late = join(
        &fixture.root.join("laptop"),
        &Code::parse(&code).unwrap(),
        fixture.local(),
    )
    .await
    .unwrap_err();
    assert!(late.to_string().contains("unknown, expired"), "{late}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_without_pairing_slots_is_named_as_such() {
    let fixture = fixture().await;
    let old = fixture.root.join("old");
    crate::pairing::private_directory(&old).unwrap();
    let (url, ca, stop) = serve(axum::Router::new(), &old).await;
    let error = join(
        &fixture.root.join("laptop"),
        &Code::parse("472-1234-5678").unwrap(),
        Join {
            relay: &url,
            proxy_url: None,
            ca_cert_file: Some(ca),
        },
    )
    .await
    .unwrap_err();
    stop.cancel();
    assert!(error.to_string().contains("cannot pair by code"), "{error}");
}

#[tokio::test]
async fn a_relay_with_pairing_turned_off_says_so_and_leaves_no_folder_behind() {
    let fixture = fixture_with(Limits {
        pair_slots: Some(0),
        ..Limits::default()
    })
    .await;
    let dir = fixture.root.join("pairing");
    let error = offer(
        &dir,
        fixture.options(),
        &fixture.client_token,
        Lifetimes::default(),
        &Default::default(),
        |_| panic!("no code without a slot"),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("turned off"), "{error}");
    assert!(!dir.exists());
}
