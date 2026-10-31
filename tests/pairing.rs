use k_lani_link::config::{Config, random_id};
use std::process::Command;

#[test]
fn two_independent_nodes_derive_the_room_from_the_same_code() {
    let binary = env!("CARGO_BIN_EXE_k-lani-link");
    let root = std::env::temp_dir().join(format!("link-pair-{}", random_id().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let code = root.join("code.txt");
    let output = Command::new(binary)
        .args(["new-code", "--out"])
        .arg(&code)
        .output()
        .unwrap();
    assert!(output.status.success() && output.stdout.is_empty() && output.stderr.is_empty());
    let mut configurations = Vec::new();
    for node in ["host", "client"] {
        let path = root.join(format!("{node}.toml"));
        let output = Command::new(binary)
            .args(["pair", "--config"])
            .arg(&path)
            .args(["--code-file"])
            .arg(&code)
            .args([
                "--signaling-url",
                "https://example.com",
                "--stun",
                "stun:stun.example.com:3478",
            ])
            .output()
            .unwrap();
        assert!(output.status.success() && output.stdout.is_empty() && output.stderr.is_empty());
        configurations.push(Config::load(&path).unwrap());
    }
    assert_eq!(configurations[0].room, configurations[0].pair_key[..12]);
    assert_eq!(configurations[0].room, configurations[1].room);
    assert_eq!(configurations[0].pair_key, configurations[1].pair_key);
    assert_ne!(configurations[0].api_key, configurations[1].api_key);
    std::fs::write(&code, "123456").unwrap();
    let bad = root.join("bad.toml");
    let output = Command::new(binary)
        .args(["pair", "--config"])
        .arg(&bad)
        .args(["--code-file"])
        .arg(&code)
        .args(["--signaling-url", "https://example.com"])
        .output()
        .unwrap();
    assert!(!output.status.success() && !bad.exists());
    // The mistake is explained, but the rejected value is never echoed.
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error: the connection code must have 43 URL-safe base64 characters\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}
