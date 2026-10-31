//! Admission check for a locally configured inference server's privacy profile.
use crate::config::Config;
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
struct Health {
    link_privacy: Option<Privacy>,
}
#[derive(Deserialize)]
struct Privacy {
    version: u8,
    application_logs: bool,
    request_history: bool,
    diagnostics: bool,
    remote_images: bool,
}

pub async fn verify(http: &reqwest::Client, config: &Config) -> Result<()> {
    verify_at(http, &config.upstream, &config.upstream_api_key).await
}

pub async fn verify_at(http: &reqwest::Client, upstream: &str, api_key: &str) -> Result<()> {
    let mut request = http
        .get(format!("{}/health", upstream.trim_end_matches('/')))
        .timeout(Duration::from_secs(5));
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let mut response = request.send().await?.error_for_status()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= 16 * 1024,
            "invalid privacy response"
        );
        bytes.extend_from_slice(&chunk);
    }
    let health: Health = serde_json::from_slice(&bytes)?;
    let private = health.link_privacy.is_some_and(|p| {
        p.version == 1
            && !p.application_logs
            && !p.request_history
            && !p.diagnostics
            && !p.remote_images
    });
    ensure!(private, "upstream privacy profile is not active");
    Ok(())
}
