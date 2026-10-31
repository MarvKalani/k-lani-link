use k_lani_link::config::random_id;
use std::{
    process::{Command, Stdio},
    time::Duration,
};

#[test]
fn cli_and_transport_failures_emit_nothing_even_with_trace_environment() {
    let binary = env!("CARGO_BIN_EXE_k-lani-link");
    let root = std::env::temp_dir().join(format!("link-no-log-{}", random_id().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let pair = root.join("pair");
    let output = Command::new(binary)
        .args(["init", "--dir"])
        .arg(&pair)
        .env("RUST_LOG", "trace")
        .env("RUST_BACKTRACE", "full")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let mut child = Command::new(binary)
        .args(["host", "--config"])
        .arg(pair.join("host.toml"))
        .env("RUST_LOG", "trace")
        .env("RUST_BACKTRACE", "full")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The rendezvous is unreachable: connection errors must stay invisible.
    std::thread::sleep(Duration::from_millis(700));
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_arguments_never_echo_the_supplied_value() {
    let binary = env!("CARGO_BIN_EXE_k-lani-link");
    let secret = "SYNTHETIC_PRIVATE_VALUE_MUST_NOT_BE_ECHOED";
    // The argument NAME (our own definition) helps; a supplied VALUE is never repeated.
    for (args, shown) in [
        (vec![secret], "unknown command"),
        (
            vec!["init", "--signaling-backend", secret],
            "--signaling-backend",
        ),
        (vec!["init", "--signaling-backend", secret], "ice-exchange"),
        (
            vec!["signal", "--token-file", "unused", "--listen", secret],
            "--listen",
        ),
        (vec!["check", "--config"], "--config"),
        (vec!["host"], "--config"),
        (vec!["host", &format!("--{secret}")], "unknown argument"),
    ] {
        let output = Command::new(binary).args(&args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(shown), "{args:?}: {error}");
        assert!(
            !error.contains(secret),
            "argument value was echoed: {error}"
        );
    }
}

#[test]
fn explicit_help_shows_the_usage_text_without_starting_the_ui() {
    let output = Command::new(env!("CARGO_BIN_EXE_k-lani-link"))
        .arg("--help")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Usage:") && text.contains("init") && text.contains("host"));
}

#[test]
fn an_unusable_rendezvous_answer_is_not_blamed_on_the_code_or_the_clock_alone() {
    use std::io::{Read, Write};
    // Something answers every request with a web page, like a captive portal or a
    // misconfigured proxy would.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = "<html>Sign in to the guest network</html>";
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let binary = env!("CARGO_BIN_EXE_k-lani-link");
    let root = std::env::temp_dir().join(format!("link-junk-{}", random_id().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let pair = root.join("pair");
    assert!(
        Command::new(binary)
            .args(["init", "--dir"])
            .arg(&pair)
            .args(["--signaling-backend", "ice-exchange", "--signaling-url"])
            .arg(format!("http://127.0.0.1:{port}"))
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new(binary)
        .args(["host", "--config"])
        .arg(pair.join("host.toml"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("rendezvous answered unexpectedly") && message.contains("signaling URL"),
        "{message}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn operator_mistakes_get_one_fixed_line_that_never_echoes_paths_or_values() {
    let binary = env!("CARGO_BIN_EXE_k-lani-link");
    let root = std::env::temp_dir().join(format!("link-diagnostics-{}", random_id().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let run = |args: &[&str]| {
        Command::new(binary)
            .args(args)
            .env("RUST_LOG", "trace")
            .output()
            .unwrap()
    };

    let usage = run(&["invalid-command"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty() && !usage.stderr.is_empty());

    let missing = run(&["check", "--config", "missing-private-config"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&missing.stderr),
        "error: configuration file not found\n"
    );

    let pair = root.join("pair");
    assert!(
        run(&["init", "--dir", pair.to_str().unwrap()])
            .status
            .success()
    );
    let host = pair.join("host.toml");
    let checked = run(&["check", "--config", host.to_str().unwrap()]);
    assert!(checked.status.success());
    assert_eq!(
        String::from_utf8_lossy(&checked.stdout),
        "configuration OK\n"
    );
    assert!(checked.stderr.is_empty(), "0600 files need no warning");

    // A broken value is located by line but its content is never printed.
    let broken = root.join("broken.toml");
    let text = std::fs::read_to_string(&host).unwrap();
    std::fs::write(
        &broken,
        text.replace(
            "request_timeout_seconds = 600",
            "request_timeout_seconds = \"leaked-secret\"",
        ),
    )
    .unwrap();
    let rejected = run(&["check", "--config", broken.to_str().unwrap()]);
    let message = String::from_utf8_lossy(&rejected.stderr);
    assert!(!rejected.status.success());
    assert!(
        message.starts_with("error: configuration file is not valid"),
        "{message}"
    );
    assert!(message.contains("see line") && !message.contains("leaked-secret"));

    // Unknown keys are user input too; a pasted credential must not be echoed as a field name.
    std::fs::write(&broken, format!("{text}\nSYNTHETIC_PRIVATE_FIELD = 1\n")).unwrap();
    let rejected = run(&["check", "--config", broken.to_str().unwrap()]);
    let message = String::from_utf8_lossy(&rejected.stderr);
    assert!(!rejected.status.success());
    assert!(message.contains("unknown field") && !message.contains("SYNTHETIC_PRIVATE_FIELD"));

    // An existing pair directory is reported without exposing anything from it.
    let again = run(&["init", "--dir", pair.to_str().unwrap()]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("create NEW pair directory"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loose = run(&["check", "--config", host.to_str().unwrap()]);
        assert!(loose.status.success());
        assert!(
            String::from_utf8_lossy(&loose.stderr)
                .starts_with("warning: configuration file is accessible")
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
