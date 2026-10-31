//! An orderly stop (SIGTERM from `docker stop` or systemd) must release the pairing
//! reservation, otherwise the next start finds the code "used" and a new code is needed.
#![cfg(unix)]

use k_lani_link::{
    config::{Config, random_id},
    pairing::{CodeUnavailable, PairingLease, state_directory},
};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const BINARY: &str = env!("CARGO_BIN_EXE_k-lani-link");

fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn reservations(state: &Path) -> usize {
    std::fs::read_dir(state).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".reserved"))
            .count()
    })
}

fn terminate(child: &mut Child) -> std::process::ExitStatus {
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let mut status = None;
    assert!(wait_until(|| {
        status = child.try_wait().unwrap();
        status.is_some()
    }));
    status.unwrap()
}

fn new_pair() -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("kl-t-{}", random_id().unwrap()));
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let pair = root.join("pair");
    assert!(
        Command::new(BINARY)
            .args(["init", "--dir"])
            .arg(&pair)
            .status()
            .unwrap()
            .success()
    );
    (root, pair)
}

#[test]
fn sigterm_releases_the_host_reservation_so_the_same_code_starts_again() {
    let (root, pair) = new_pair();
    let config_path = pair.join("host.toml");
    let config = Config::load(&config_path).unwrap();
    let state = state_directory(&config, &config_path);
    let mut host = Command::new(BINARY)
        .args(["host", "--config"])
        .arg(&config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(
        wait_until(|| reservations(&state) == 1),
        "host must hold its lease"
    );
    // Not yet stopped: a second claim on the code is refused.
    assert!(
        PairingLease::reserve(&state, &config.pair_key)
            .err()
            .unwrap()
            .is::<CodeUnavailable>()
    );

    assert!(terminate(&mut host).success(), "SIGTERM is an orderly stop");
    let output = host.wait_with_output().unwrap();
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert_eq!(
        reservations(&state),
        0,
        "stale reservation would burn the code"
    );
    drop(PairingLease::reserve(&state, &config.pair_key).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn sigterm_releases_the_client_reservation_and_removes_its_api_socket() {
    let (root, pair) = new_pair();
    let config_path = pair.join("client.toml");
    let config = Config::load(&config_path).unwrap();
    let state = state_directory(&config, &config_path);
    let socket = root.join("api.sock");
    let mut client = Command::new(BINARY)
        .args(["client", "--config"])
        .arg(&config_path)
        .arg("--api-socket")
        .arg(&socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(wait_until(|| reservations(&state) == 1 && socket.exists()));

    assert!(terminate(&mut client).success());
    let output = client.wait_with_output().unwrap();
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    assert_eq!(reservations(&state), 0);
    assert!(!socket.exists(), "API socket must not outlive the process");
    drop(PairingLease::reserve(&state, &config.pair_key).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stale_reservation_is_reported_instead_of_failing_silently() {
    let (root, pair) = new_pair();
    let config_path = pair.join("host.toml");
    let config = Config::load(&config_path).unwrap();
    let state = state_directory(&config, &config_path);
    // Simulates an earlier run that was killed with SIGKILL.
    std::mem::forget(PairingLease::reserve(&state, &config.pair_key).unwrap());
    let output = Command::new(BINARY)
        .args(["host", "--config"])
        .arg(&config_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.starts_with("error: connection code is used or already reserved"),
        "{message}"
    );
    assert!(!message.contains(&config.pair_key));
    std::fs::remove_dir_all(root).unwrap();
}
