//! Host only: the operator's model catalog behind `model_control_socket`, which model of it
//! is loaded, and loading another one when a request names it (`load_on_demand`).
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

/// How many requests for the loaded model may overtake one that waits for another model.
/// Together with `OVERTAKE_SECONDS` this groups requests per model without starving any.
pub const OVERTAKE_LIMIT: usize = 8;
pub const OVERTAKE_SECONDS: u64 = 30;
/// A catalog read never waits for a model to load: it is answered from the last snapshot.
const SNAPSHOT_SECONDS: u64 = 5;
#[cfg(unix)]
const REPLY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    pub active_model: Option<String>,
    #[serde(default)]
    pub models: Vec<Model>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    /// Missing from supervisors before llama.cpp support: those only run Basalt.
    #[serde(default)]
    pub engine: Option<String>,
    #[serde(default)]
    pub context_per_slot: Option<u64>,
    #[serde(default)]
    pub loaded: bool,
    /// `basalt`: the engine's own privacy profile passed. `supervised`: the supervisor
    /// started an engine without logs, web UI, slot monitor or network.
    #[serde(default)]
    pub privacy: Option<String>,
}
impl Snapshot {
    pub fn lists(&self, model: &str) -> bool {
        self.models.iter().any(|m| m.id == model)
    }
    /// Every allowlisted model in the `GET /v1/models` shape; `loaded` tells which one is.
    pub fn entries(&self) -> Vec<Value> {
        self.models
            .iter()
            .map(|m| {
                let mut entry = json!({"id": m.id, "object": "model", "owned_by": "k-lani-link", "loaded": m.loaded});
                if let Some(context) = m.context_per_slot {
                    entry["context_length"] = json!(context);
                }
                entry
            })
            .collect()
    }
}

/// The model a request body names, if it is JSON with a string `model`.
pub fn requested(body: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct Request {
        model: Option<String>,
    }
    serde_json::from_slice::<Request>(body).ok()?.model
}

#[cfg(unix)]
async fn call(socket: &Path, post: Option<Value>, timeout: Option<Duration>) -> Result<Snapshot> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .unix_socket(socket)
        .build()?;
    let mut request = match &post {
        Some(body) => client
            .post("http://localhost/v1/link/model")
            .header("content-type", "application/json")
            .body(serde_json::to_vec(body)?),
        None => client.get("http://localhost/v1/link/models"),
    };
    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }
    let mut response = request.send().await?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            bytes.len() + chunk.len() <= REPLY_BYTES,
            "model supervisor reply too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        // The supervisor's reasons are fixed texts and name no paths.
        let reason = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "model supervisor refused".into());
        bail!(reason);
    }
    Ok(serde_json::from_slice(&bytes)?)
}
#[cfg(not(unix))]
async fn call(_: &Path, _: Option<Value>, _: Option<Duration>) -> Result<Snapshot> {
    bail!("model management needs Unix")
}

pub async fn snapshot(socket: &Path) -> Result<Snapshot> {
    call(socket, None, Some(Duration::from_secs(SNAPSHOT_SECONDS))).await
}

/// Loads `model`, unloading the current one first. Takes as long as the engine needs.
pub async fn load(socket: &Path, model: &str) -> Result<Snapshot> {
    call(
        socket,
        Some(json!({"action": "load", "model": model})),
        None,
    )
    .await
}

/// The privacy check for the managed backend: the loaded engine's own profile for Basalt,
/// the supervisor's account of how it started the engine for llama.cpp.
pub async fn verify(
    socket: &Path,
    http: &reqwest::Client,
    upstream: &str,
    api_key: &str,
) -> Result<()> {
    let snapshot = snapshot(socket).await?;
    let active = snapshot
        .active_model
        .as_deref()
        .and_then(|id| snapshot.models.iter().find(|m| m.id == id));
    match active.map(|m| (m.engine.as_deref(), m.privacy.as_deref())) {
        Some((_, Some("supervised"))) => Ok(()),
        Some((_, Some("basalt")) | (None, None)) => {
            crate::backend::verify_at(http, upstream, api_key).await
        }
        _ => bail!("upstream privacy profile is not active"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_lists_every_allowlisted_model_and_marks_the_loaded_one() {
        let snapshot: Snapshot = serde_json::from_value(json!({
            "active_model": "coder",
            "models": [
                {"id": "coder", "engine": "basalt", "context_per_slot": 262144, "loaded": true, "privacy": "basalt"},
                {"id": "Mellum2.1-Q8_0", "engine": "llama", "context_per_slot": 131072, "loaded": false, "privacy": null}
            ]
        }))
        .unwrap();
        let entries = snapshot.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["id"], "Mellum2.1-Q8_0");
        assert_eq!(entries[1]["loaded"], false);
        assert_eq!(entries[0]["context_length"], 262144);
        assert!(snapshot.lists("coder") && !snapshot.lists("other"));
        assert_eq!(
            requested(br#"{"model":"coder","messages":[]}"#).as_deref(),
            Some("coder")
        );
        assert_eq!(requested(b"not json"), None);
    }
}
