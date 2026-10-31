//! Exported credentials must be portable, durable, private and never silently reset.
use k_lani_link::{
    chain_state::Store,
    config::{Config, Transport, private_file, random_id, random_secret},
};
use k_lani_link_protocol::EndpointRole;
use std::{path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("link-issue-{}", random_id().unwrap()));
        k_lani_link::pairing::private_directory(&root).unwrap();
        private_file(&root.join("token"), &random_secret().unwrap()).unwrap();
        Self(root)
    }
    fn issue(&self) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_k-lani-link"))
            .args(["init-https", "--dir"])
            .arg(self.0.join("pair"))
            .args(["--relay-url", "https://localhost", "--admission-token-file"])
            .arg(self.0.join("token"))
            .env("RUST_LOG", "trace")
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn issuance_exports_both_durable_roles_and_moving_the_client_preserves_state() {
    let fixture = Fixture::new();
    let output = fixture.issue();
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let pair = fixture.0.join("pair");
    let host = Config::load(&pair.join("host/host.toml")).unwrap();
    let client_path = pair.join("client/client.toml");
    let text = std::fs::read_to_string(&client_path).unwrap();
    assert!(!text.contains("pair_key"));
    assert!(text.contains("state_dir = \"state\""));
    let client = Config::load(&client_path).unwrap();
    assert!(host.transport == Transport::Https && client.transport == Transport::Https);
    let initial = {
        let store = Store::open(&client.https.unwrap().state_dir, EndpointRole::Client).unwrap();
        store
            .client(k_lani_link::signal::now())
            .unwrap()
            .chain
            .room()
    };
    {
        let store = Store::open(&host.https.unwrap().state_dir, EndpointRole::Host).unwrap();
        assert_eq!(
            store
                .host(k_lani_link::signal::now())
                .unwrap()
                .chains
                .current()
                .room(),
            initial
        );
    }
    let moved = fixture.0.join("moved");
    std::fs::rename(pair.join("client"), &moved).unwrap();
    let client = Config::load(&moved.join("client.toml")).unwrap();
    let options = client.https.unwrap();
    // Loading resolves the folder, so compare resolved paths (/private/var on macOS,
    // \\?\ and long names on Windows).
    assert_eq!(
        std::fs::canonicalize(&options.state_dir).unwrap(),
        std::fs::canonicalize(moved.join("state")).unwrap()
    );
    let store = Store::open(&options.state_dir, EndpointRole::Client).unwrap();
    assert_eq!(
        store
            .client(k_lani_link::signal::now())
            .unwrap()
            .chain
            .room(),
        initial
    );
    assert!(k_lani_link::mailbox::http::Http::new(&options).is_ok());
    let before = std::fs::read(moved.join("state/client.chain")).unwrap();
    assert!(!fixture.issue().status.success());
    assert_eq!(
        before,
        std::fs::read(moved.join("state/client.chain")).unwrap()
    );
}

#[test]
fn revocation_refuses_live_or_wrong_role_state_and_startup_never_bootstraps_missing_state() {
    let fixture = Fixture::new();
    assert!(fixture.issue().status.success());
    let folder = fixture.0.join("pair/client");
    let config = folder.join("client.toml");
    let file = folder.join("state/client.chain");
    let before = std::fs::read(&file).unwrap();
    let revoke = |role| {
        Command::new(env!("CARGO_BIN_EXE_k-lani-link"))
            .args(["revoke-https", "--role", role, "--config"])
            .arg(&config)
            .output()
            .unwrap()
    };
    assert!(!revoke("host").status.success());
    assert_eq!(before, std::fs::read(&file).unwrap());
    {
        let _locked = Store::open(&folder.join("state"), EndpointRole::Client).unwrap();
        assert!(!revoke("client").status.success());
        assert_eq!(before, std::fs::read(&file).unwrap());
    }
    let output = revoke("client");
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    {
        let store = Store::open(&folder.join("state"), EndpointRole::Client).unwrap();
        assert!(store.client(k_lani_link::signal::now()).is_err());
    }
    std::fs::remove_file(&file).unwrap();
    let store = Store::open(&folder.join("state"), EndpointRole::Client).unwrap();
    assert!(store.client(k_lani_link::signal::now()).is_err());
    assert!(!file.exists());
}

#[test]
fn https_configuration_rejects_mixed_direct_transport_and_unbounded_relay_settings() {
    let fixture = Fixture::new();
    assert!(fixture.issue().status.success());
    let mut config = Config::load(&fixture.0.join("pair/client/client.toml")).unwrap();
    config.include_loopback_candidate = true;
    assert!(config.validate().is_err());
    config.include_loopback_candidate = false;
    config.https.as_mut().unwrap().poll_wait_seconds = 26;
    assert!(config.validate().is_err());
    config.https.as_mut().unwrap().poll_wait_seconds = 0;
    assert!(config.validate().is_ok());
    let settings = r#"listen = "0.0.0.0:8789"
tokens = [{ token_file = "token" }]
"#;
    private_file(&fixture.0.join("relay.toml"), settings).unwrap();
    assert!(k_lani_link::mailbox::server::Settings::load(&fixture.0.join("relay.toml")).is_err());
}

#[test]
fn the_backend_socket_goes_into_the_host_folder_only() {
    let fixture = Fixture::new();
    let output = Command::new(env!("CARGO_BIN_EXE_k-lani-link"))
        .args(["init-https", "--dir"])
        .arg(fixture.0.join("pair"))
        .args(["--relay-url", "https://localhost", "--admission-token-file"])
        .arg(fixture.0.join("token"))
        .args(["--upstream", "http://localhost"])
        .args(["--upstream-socket", "/run/link/basalt.sock"])
        .output()
        .unwrap();
    // A backend socket is Unix only; elsewhere the setup refuses it.
    assert_eq!(output.status.success(), cfg!(unix));
    if !cfg!(unix) {
        return;
    }
    let host = Config::load(&fixture.0.join("pair/host/host.toml")).unwrap();
    assert_eq!(host.upstream, "http://localhost");
    assert_eq!(
        host.upstream_socket,
        Some(PathBuf::from("/run/link/basalt.sock"))
    );
    let client = std::fs::read_to_string(fixture.0.join("pair/client/client.toml")).unwrap();
    assert!(!client.contains("basalt.sock"));
}
