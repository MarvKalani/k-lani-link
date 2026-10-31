//! Explicit opt-in check of the existing rendezvous. No inference or real prompts.
use k_lani_link::{
    config::{Config, RelayPolicy, SignalingBackend, random_secret},
    peer,
};
use k_lani_link_protocol::Frame;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an explicitly selected live signaling service"]
async fn encrypted_pairing_with_selected_live_exchange() {
    let code = random_secret().unwrap();
    let config = Config {
        transport: Default::default(),
        https: None,
        signaling_url: std::env::var("LINK_LIVE_SIGNAL_URL").expect("select a signaling URL"),
        signaling_backend: SignalingBackend::IceExchange,
        signaling_token: String::new(),
        room: k_lani_link_protocol::room_from_key(&code).unwrap(),
        pair_key: code,
        pairing_state_dir: None,
        api_key: random_secret().unwrap(),
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
    let host_config = config.clone();
    let host = tokio::spawn(async move { peer::accept(&host_config, &mut String::new()).await });
    let client = tokio::time::timeout(Duration::from_secs(70), peer::connect(&config))
        .await
        .unwrap()
        .unwrap();
    let mut host = tokio::time::timeout(Duration::from_secs(5), host)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    client
        .tx
        .send(Frame::Request {
            id: 7,
            method: "GET".into(),
            path: "/v1/models".into(),
        })
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), host.rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(frame, Frame::Request { id: 7, .. }));
    client.close().await;
    host.close().await;
}
