use k_lani_link::config::{self, Config};
use std::process::Command;

#[test]
fn profile_contains_no_credentials_and_the_helper_delivers_only_the_local_api_token() {
    let binary = env!("CARGO_BIN_EXE_k-lani-link");
    let root = std::env::temp_dir().join(format!("link-codex-{}", config::random_id().unwrap()));
    config::generate_pair(&root, "http://127.0.0.1:8788", Default::default(), vec![]).unwrap();
    let config = root.join("client.toml");
    let cfg = Config::load(&config).unwrap();
    let output = root.join("link.config.toml");
    let make = || {
        Command::new(binary)
            .arg("codex-profile")
            .arg("--config")
            .arg(&config)
            .arg("--output")
            .arg(&output)
            .args(["--model", "qwen3.8-flash-next"])
            .output()
            .unwrap()
    };
    let result = make();
    assert!(result.status.success() && result.stdout.is_empty() && result.stderr.is_empty());
    let profile = std::fs::read_to_string(&output).unwrap();
    assert!(!profile.contains(&cfg.api_key) && !profile.contains(&cfg.pair_key));
    let settings: toml::Value = toml::from_str(&profile).unwrap();
    assert_eq!(settings["model_provider"].as_str(), Some("link"));
    assert_eq!(settings["web_search"].as_str(), Some("disabled"));
    assert_eq!(
        settings["model_providers"]["link"]["stream_max_retries"].as_integer(),
        Some(0)
    );
    let catalog = std::fs::read_to_string(output.with_extension("models.json")).unwrap();
    let models: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert_eq!(models["models"][0]["slug"], "qwen3.8-flash-next");
    assert!(!catalog.contains(&cfg.api_key) && !catalog.contains(&cfg.pair_key));
    assert!(
        !make().status.success(),
        "existing configuration must not be overwritten"
    );
    assert_eq!(std::fs::read_to_string(&output).unwrap(), profile);
    let token = Command::new(binary)
        .arg("codex-token")
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(token.status.success() && token.stderr.is_empty());
    assert!(String::from_utf8(token.stdout).unwrap().trim() == cfg.api_key);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn socket_is_private_and_does_not_remove_an_existing_file() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("kl-s-{}", config::random_id().unwrap()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = root.join("api.sock");
    std::fs::write(&path, "keep").unwrap();
    assert!(k_lani_link::local_api::Socket::bind(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep");
    std::fs::remove_file(&path).unwrap();
    let (guard, listener) = k_lani_link::local_api::Socket::bind(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(listener);
    drop(guard);
    assert!(!path.exists());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(k_lani_link::local_api::Socket::bind(&path).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
