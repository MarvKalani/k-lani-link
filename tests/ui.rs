//! Native control-panel lifecycle, including the actual packaged executable.
use std::{
    process::{Command, Stdio},
    time::Duration,
};

#[tokio::test]
async fn native_panel_serves_local_ui_reopens_one_instance_and_quits_without_logging_secrets() {
    let directory = std::env::temp_dir().join(format!(
        "link-ui-process-{}",
        k_lani_link::config::random_id().unwrap()
    ));
    let mut child = Command::new(env!("CARGO_BIN_EXE_k-lani-link"))
        .args(["ui", "--no-open", "--data-dir"])
        .arg(&directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let address_file = directory.join("panel-address.txt");
    let address = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(&address_file) {
                break text;
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "panel exited before startup"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let url = url::Url::parse(&address).unwrap();
    let token = url.fragment().unwrap().to_owned();
    let base = format!("http://127.0.0.1:{}", url.port().unwrap());
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let page = http.get(&base).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("Test backend") && page.contains("client.toml"));
    assert!(!page.contains(&token));
    let reopened = Command::new(env!("CARGO_BIN_EXE_k-lani-link"))
        .args(["ui", "--no-open", "--data-dir"])
        .arg(&directory)
        .output()
        .unwrap();
    assert!(reopened.status.success() && reopened.stdout.is_empty() && reopened.stderr.is_empty());
    assert_eq!(std::fs::read_to_string(&address_file).unwrap(), address);
    let status: serde_json::Value = http
        .get(format!("{base}/api/status"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["phase"], "not_configured");
    assert!(
        http.post(format!("{base}/api/quit"))
            .bearer_auth(&token)
            .json(&serde_json::json!({}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success() && output.stdout.is_empty() && output.stderr.is_empty());
    assert!(!address_file.exists());
    std::fs::remove_dir_all(directory).unwrap();
}
