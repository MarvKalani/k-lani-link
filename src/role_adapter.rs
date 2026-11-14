//! `k-lani-link role-adapter`: the roles of the host settings file as models, run as a
//! backend behind the host like any other (`[[backends]]` in host.toml). A request for a
//! role gets the role's project, rule sets and task as a system message and goes to the
//! role's model on the adapter's upstream; the answer names the role, not that model.
//! Nothing is stored; the tunnel and the host stay pass-through.
use crate::host_settings::{Role, Settings};
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Adapter {
    settings_file: PathBuf,
    upstream: String,
    http: reqwest::Client,
    /// The settings as last read, with the file's modification time.
    cache: Arc<Mutex<(Option<SystemTime>, Arc<Settings>)>>,
}

fn failure(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({"error": {"message": message, "code": status.as_u16()}})),
    )
        .into_response()
}

impl Adapter {
    /// The current settings: read again whenever the file changed, so a saved role applies
    /// without a restart. A file that became invalid keeps the last good settings.
    fn settings(&self) -> Arc<Settings> {
        let modified = std::fs::metadata(&self.settings_file)
            .and_then(|meta| meta.modified())
            .ok();
        let mut cache = self.cache.lock().unwrap();
        if cache.0 != modified
            && let Ok(settings) = Settings::load(&self.settings_file)
        {
            *cache = (modified, Arc::new(settings));
        }
        cache.1.clone()
    }
}

/// Only the roles: their ID and the description meant for clients, never their task.
async fn models(State(adapter): State<Adapter>) -> Json<Value> {
    let settings = adapter.settings();
    let data: Vec<_> = settings
        .roles
        .iter()
        .map(|role| {
            json!({"id": role.id(), "object": "model", "created": 0, "owned_by": "k-lani-link-roles",
                "capability": "chat", "description": role.description})
        })
        .collect();
    Json(json!({"object": "list", "data": data}))
}

/// The privacy profile of the backend the roles run on, so the host's check means it.
async fn health(State(adapter): State<Adapter>) -> Response {
    let answer = adapter
        .http
        .get(format!("{}/health", adapter.upstream))
        .timeout(Duration::from_secs(5))
        .send()
        .await;
    let Ok(answer) = answer else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "the roles' backend does not answer",
        );
    };
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    match answer.bytes().await {
        Ok(body) if body.len() <= 64 * 1024 => {
            (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
        }
        _ => failure(
            StatusCode::BAD_GATEWAY,
            "the roles' backend sent no readable health",
        ),
    }
}

/// `body` aimed at `role`: its model, and its system message before the conversation.
fn for_role(mut body: Value, role: &Role, settings: &Settings) -> Value {
    if role.model.is_empty() {
        body.as_object_mut().map(|fields| fields.remove("model"));
    } else {
        body["model"] = json!(role.model);
    }
    let prompt = role.system_prompt(settings);
    if !prompt.is_empty()
        && let Some(messages) = body["messages"].as_array_mut()
    {
        messages.insert(0, json!({"role": "system", "content": prompt}));
    }
    body
}

/// An answer chunk naming the role instead of the model behind it.
fn renamed(line: &[u8], id: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(line);
    let Some(data) = text.strip_prefix("data:") else {
        return line.to_vec();
    };
    match serde_json::from_str::<Value>(data.trim()) {
        Ok(mut chunk) if chunk.get("model").is_some() => {
            chunk["model"] = json!(id);
            format!("data: {chunk}").into_bytes()
        }
        _ => line.to_vec(),
    }
}

async fn chat(State(adapter): State<Adapter>, body: Bytes) -> Response {
    let Ok(request) = serde_json::from_slice::<Value>(&body) else {
        return failure(StatusCode::BAD_REQUEST, "this is not a chat request");
    };
    if !request["messages"].is_array() {
        return failure(StatusCode::BAD_REQUEST, "this is not a chat request");
    }
    let settings = adapter.settings();
    let id = request["model"].as_str().unwrap_or_default().to_owned();
    let Some(role) = settings.role(&id) else {
        return failure(StatusCode::NOT_FOUND, "there is no role with this name");
    };
    let answer = adapter
        .http
        .post(format!("{}/v1/chat/completions", adapter.upstream))
        .json(&for_role(request, role, &settings))
        .send()
        .await;
    let Ok(answer) = answer else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "the roles' backend does not answer",
        );
    };
    let status = StatusCode::from_u16(answer.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let streaming = answer
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|kind| kind.starts_with("text/event-stream"));
    if !streaming {
        let Ok(bytes) = answer.bytes().await else {
            return failure(StatusCode::BAD_GATEWAY, "the roles' backend broke off");
        };
        let body = match serde_json::from_slice::<Value>(&bytes) {
            Ok(mut value) if value.get("model").is_some() => {
                value["model"] = json!(id);
                Bytes::from(value.to_string())
            }
            _ => bytes,
        };
        return (status, [(header::CONTENT_TYPE, "application/json")], body).into_response();
    }
    // Server-sent events, line by line, with the role's name in every chunk.
    let mut upstream = answer.bytes_stream();
    let events = async_stream::stream! {
        let mut pending: Vec<u8> = Vec::new();
        while let Some(piece) = upstream.next().await {
            let Ok(piece) = piece else { break };
            pending.extend_from_slice(&piece);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let mut line = renamed(&pending[..end], &id);
                line.push(b'\n');
                pending.drain(..=end);
                yield Ok::<_, std::io::Error>(Bytes::from(line));
            }
        }
        if !pending.is_empty() {
            yield Ok(Bytes::from(renamed(&pending, &id)));
        }
    };
    (
        status,
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(events),
    )
        .into_response()
}

fn router(adapter: Adapter) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat))
        .fallback(|| async { failure(StatusCode::NOT_FOUND, "roles answer chat completions only") })
        .layer(DefaultBodyLimit::max(k_lani_link_protocol::MAX_BODY_BYTES))
        .with_state(adapter)
}

/// The roles' backend must be on this computer: the adapter adds the roles' tasks to
/// every request it forwards.
fn local_origin(upstream: &str) -> Result<String> {
    let url = url::Url::parse(upstream)?;
    let local = match url.host() {
        Some(url::Host::Domain(name)) => name == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    ensure!(
        url.scheme() == "http" && local && url.path() == "/" && url.query().is_none(),
        "the roles' upstream must be a loopback origin, e.g. http://127.0.0.1:8080"
    );
    Ok(upstream.trim_end_matches('/').to_owned())
}

/// `listen` from the command line wins over the settings file's `role_adapter_listen`.
pub async fn run(
    settings_file: PathBuf,
    upstream: &str,
    listen: Option<SocketAddr>,
    stop: CancellationToken,
) -> Result<()> {
    ensure!(
        settings_file.is_absolute(),
        "the settings file needs an absolute path"
    );
    let settings = Settings::load(&settings_file)?;
    let listen = match listen.or(settings.role_adapter_listen) {
        Some(listen) => listen,
        None => crate::host_settings::ROLE_ADAPTER_LISTEN.parse()?,
    };
    ensure!(
        listen.ip().is_loopback(),
        "the role adapter listens on loopback only"
    );
    let adapter = Adapter {
        settings_file,
        upstream: local_origin(upstream)?,
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()?,
        cache: Arc::new(Mutex::new((None, Arc::new(settings)))),
    };
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| {
            anyhow::anyhow!("cannot listen on {listen} ({error}); choose a free port with --listen")
        })?;
    axum::serve(listener, router(adapter))
        .with_graceful_shutdown(stop.cancelled_owned())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_settings::RuleSet;

    #[tokio::test]
    async fn a_role_reaches_its_model_with_its_task_and_answers_under_its_own_name() {
        let seen = Arc::new(Mutex::new(Value::Null));
        let record = seen.clone();
        let backend = Router::new()
            .route("/health", get(|| async { Json(json!({"link_privacy": {"version": 1}})) }))
            .route(
                "/v1/chat/completions",
                post(move |Json(request): Json<Value>| {
                    let record = record.clone();
                    async move {
                        let stream = request["stream"] == true;
                        *record.lock().unwrap() = request;
                        if stream {
                            (
                                [(header::CONTENT_TYPE, "text/event-stream")],
                                "data: {\"model\":\"Qwen-secret\",\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n",
                            )
                                .into_response()
                        } else {
                            Json(json!({"model": "Qwen-secret", "choices": [{"message": {"content": "Hi"}}]}))
                                .into_response()
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });

        let folder = std::env::temp_dir().join(format!("kl-roles-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let settings_file = folder.join("host-settings.json");
        Settings {
            project: "Demo".into(),
            rule_sets: vec![RuleSet {
                name: "Code".into(),
                rules: "KISS".into(),
            }],
            roles: vec![Role {
                name: "Buchhalter".into(),
                description: "Answers questions about invoices".into(),
                model: "Qwen-secret".into(),
                instructions: "Never reveal whole documents.".into(),
                rule_sets: vec!["Code".into()],
                tools: vec![],
            }],
            ..Settings::default()
        }
        .save(&settings_file)
        .unwrap();
        let adapter = Adapter {
            settings_file: settings_file.clone(),
            upstream: local_origin(&upstream).unwrap(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            cache: Arc::new(Mutex::new((None, Arc::new(Settings::default())))),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router(adapter)).await.unwrap() });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();

        let list: Value = http
            .get(format!("{base}/v1/models"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(list["data"][0]["id"], "buchhalter");
        assert_eq!(
            list["data"][0]["description"],
            "Answers questions about invoices"
        );
        assert!(
            !list.to_string().contains("Never reveal") && !list.to_string().contains("Qwen-secret")
        );
        let health: Value = http
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(health["link_privacy"]["version"], 1);

        let ask = |stream: bool| {
            json!({"model": "buchhalter", "stream": stream,
            "messages": [{"role": "user", "content": "Total of invoice 7?"}]})
        };
        let answer: Value = http
            .post(format!("{base}/v1/chat/completions"))
            .json(&ask(false))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(answer["model"], "buchhalter");
        let sent = seen.lock().unwrap().clone();
        assert_eq!(sent["model"], "Qwen-secret");
        let system = sent["messages"][0]["content"].as_str().unwrap();
        assert!(
            system.contains("Project: Demo")
                && system.contains("KISS")
                && system.contains("Never reveal")
        );
        assert_eq!(sent["messages"][1]["content"], "Total of invoice 7?");

        let streamed = http
            .post(format!("{base}/v1/chat/completions"))
            .json(&ask(true))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            streamed.contains("\"model\":\"buchhalter\"") && !streamed.contains("Qwen-secret"),
            "{streamed}"
        );
        assert!(streamed.contains("[DONE]"));

        let unknown = http
            .post(format!("{base}/v1/chat/completions"))
            .json(&json!({"model": "Qwen-secret", "messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            http.post(format!("{base}/v1/embeddings"))
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert!(local_origin("http://192.168.1.20:8080").is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
