//! Local browser control panel. No telemetry, request logging or inference transcript storage.
use crate::{
    config::{Config, RelayPolicy, private_file, random_secret, token_matches},
    diagnostic::Diagnostic,
    gateway::Gateway,
    pairing::private_directory,
    shutdown,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    future::IntoFuture,
    net::SocketAddr,
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::Instant,
};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;

const HTML: &str = include_str!("ui/index.html");
const JS: &str = include_str!("ui/app.js");
const CSS: &str = include_str!("ui/style.css");

struct Active {
    stop: CancellationToken,
    task: JoinHandle<()>,
}
#[derive(Default)]
struct Control {
    config: Option<Config>,
    active: Option<Active>,
    /// A pairing by code is waiting for the other computer.
    joining: bool,
}
#[derive(Clone)]
struct Panel {
    token: String,
    origin: String,
    directory: PathBuf,
    control: Arc<Mutex<Control>>,
    stop: CancellationToken,
    http: reqwest::Client,
    /// A private CA for the relay; only tests set one, a real relay needs a public certificate.
    relay_ca: Option<PathBuf>,
}

fn failure(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

async fn protect(State(panel): State<Panel>, request: Request, next: Next) -> Response {
    let host = panel.origin.strip_prefix("http://").unwrap_or_default();
    if request.headers().get("host").and_then(|v| v.to_str().ok()) != Some(host) {
        return failure(StatusCode::FORBIDDEN, "Unrecognized local address.");
    }
    if request
        .headers()
        .get("origin")
        .is_some_and(|v| v != panel.origin.as_str())
    {
        return failure(
            StatusCode::FORBIDDEN,
            "Requests must come from this control panel.",
        );
    }
    if request.uri().path().starts_with("/api/")
        && !token_matches(
            request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            &panel.token,
        )
    {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Open the control panel from the application.",
        );
    }
    let mut response = next.run(request).await;
    for (name, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

fn router(panel: Panel) -> Router {
    Router::new()
        .route("/", get(|| async { Html(HTML) }))
        .route(
            "/app.js",
            get(|| async { ([("content-type", "text/javascript; charset=utf-8")], JS) }),
        )
        .route(
            "/style.css",
            get(|| async { ([("content-type", "text/css; charset=utf-8")], CSS) }),
        )
        .route("/api/status", get(status))
        .route("/api/config", post(save))
        .route("/api/join", post(join))
        .route("/api/connect", post(connect))
        .route("/api/disconnect", post(disconnect))
        .route("/api/test", post(test_backend))
        .route("/api/credentials", post(credentials))
        .route("/api/quit", post(quit))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(middleware::from_fn_with_state(panel.clone(), protect))
        .with_state(panel)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    toml: String,
}

fn imported(text: &str, directory: &std::path::Path) -> anyhow::Result<Config> {
    let mut config: Config = toml::from_str(text)
        .map_err(|_| Diagnostic::err("This is not a valid client.toml configuration."))?;
    if config.transport == crate::config::Transport::Https {
        return Err(Diagnostic::err(
            "The HTTPS preview uses client --config with its private state folder. Paired-state bundle import is not yet enabled in this panel.",
        ));
    }
    config.room = k_lani_link_protocol::room_from_key(&config.pair_key)
        .map_err(|_| Diagnostic::err("The pairing code must contain 43 URL-safe characters."))?;
    if config.relay != RelayPolicy::Disabled {
        return Err(Diagnostic::err(
            "This first control-panel version supports direct connections only. TURN must be disabled.",
        ));
    }
    // Imported paths never grant this panel access to another file or another state store.
    config.pairing_state_dir = Some(directory.join("pairing-state"));
    config.management_token_file = None;
    config.model_control_socket = None;
    config.upstream_socket = None;
    config.upstream_api_key.clear();
    config.upstream = "http://127.0.0.1:8080".into();
    config.allow_unverified_backend = false;
    config.include_loopback_candidate = false;
    if config.listen.port() == 0 {
        return Err(Diagnostic::err("Choose a fixed nonzero loopback API port."));
    }
    config.validate()?;
    Ok(config)
}

async fn save(
    State(panel): State<Panel>,
    input: Result<Json<Import>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let input = match input {
        Ok(Json(input)) => input,
        Err(error) => {
            return failure(
                error.status(),
                "Submit one client.toml file smaller than 60 KiB.",
            );
        }
    };
    let mut control = panel.control.lock().await;
    if control.active.is_some() {
        return failure(
            StatusCode::CONFLICT,
            "Disconnect before replacing the configuration. A new tunnel needs a fresh pairing code.",
        );
    }
    let config = match imported(&input.toml, &panel.directory) {
        Ok(config) => config,
        Err(_) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "Configuration rejected. Import a fresh client.toml with a valid pairing/API key, loopback API address, HTTPS signaling and TURN disabled.",
            );
        }
    };
    let destination = panel.directory.join("client.toml");
    let Ok(nonce) = random_secret() else {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Cannot access secure operating-system randomness.",
        );
    };
    let temporary = panel.directory.join(format!("config-{nonce}.tmp"));
    let saved = toml::to_string_pretty(&config).ok().is_some_and(|text| {
        private_file(&temporary, &text).is_ok() && std::fs::rename(&temporary, &destination).is_ok()
    });
    if !saved {
        let _ = std::fs::remove_file(temporary);
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Cannot save the private client configuration.",
        );
    }
    // An imported file replaces a pairing by code as the configuration; its folder stays.
    let _ = std::fs::remove_file(panel.directory.join(PAIRING_POINTER));
    control.config = Some(config);
    Json(json!({"saved": true})).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinInput {
    code: String,
    #[serde(default)]
    relay: String,
}
/// Names the folder of the current HTTPS pairing inside the panel directory.
const PAIRING_POINTER: &str = "https-pairing.txt";

/// The HTTPS pairing the panel uses, from its pointer file, if any.
fn paired(directory: &std::path::Path) -> Option<Config> {
    let name = std::fs::read_to_string(directory.join(PAIRING_POINTER)).ok()?;
    let name = name.trim();
    if !name.starts_with("https-") || !name[6..].bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Config::load(&directory.join(name).join("client.toml"))
        .ok()
        .filter(|config| config.transport == crate::config::Transport::Https)
}

/// Pairing by code: receives a client folder from the host and makes it the configuration.
/// Each pairing gets a new folder; an earlier one is kept, never overwritten.
async fn join(
    State(panel): State<Panel>,
    input: Result<Json<JoinInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return failure(StatusCode::BAD_REQUEST, "Enter the pairing code.");
    };
    let code = match crate::mailbox::couple::Code::parse(&input.code) {
        Ok(code) => code,
        Err(_) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "A pairing code has eleven digits, e.g. 472-1234-5678.",
            );
        }
    };
    let relay = match input.relay.trim() {
        "" => match crate::mailbox::couple::DEFAULT_RELAY {
            Some(relay) => relay.to_owned(),
            None => return failure(StatusCode::BAD_REQUEST, "Enter the relay address."),
        },
        relay => relay.to_owned(),
    };
    {
        let mut control = panel.control.lock().await;
        if control.active.is_some() {
            return failure(StatusCode::CONFLICT, "Disconnect before pairing again.");
        }
        if control.joining {
            return failure(StatusCode::CONFLICT, "A pairing is already waiting.");
        }
        control.joining = true;
    }
    // A task of its own: closing the tab while the other computer confirms neither aborts
    // the pairing nor leaves the panel waiting forever. `status` shows the outcome.
    let worker = panel.clone();
    let task = tokio::spawn(async move {
        let result = async {
            let name = format!("https-{}", crate::config::random_id()?);
            let config = crate::mailbox::couple::join(
                &worker.directory.join(&name),
                &code,
                crate::mailbox::couple::Join {
                    relay: &relay,
                    proxy_url: None,
                    ca_cert_file: worker.relay_ca.clone(),
                },
            )
            .await?;
            let config = Config::load(&config)?;
            let temporary = worker
                .directory
                .join(format!("{PAIRING_POINTER}-{}.tmp", random_secret()?));
            private_file(&temporary, &name)?;
            std::fs::rename(&temporary, worker.directory.join(PAIRING_POINTER))?;
            anyhow::Ok(config)
        }
        .await;
        let mut control = worker.control.lock().await;
        control.joining = false;
        match result {
            Ok(config) => {
                control.config = Some(config);
                Ok(())
            }
            Err(error) => Err(crate::diagnostic::printable(&error)
                .unwrap_or_else(|| "Pairing failed. Ask for a new code and try again.".into())),
        }
    });
    match task.await {
        Ok(Ok(())) => Json(json!({"paired": true})).into_response(),
        Ok(Err(message)) => {
            (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response()
        }
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Pairing stopped unexpectedly.",
        ),
    }
}

async fn limited_body(mut response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    let mut result = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if chunk.len() > limit.saturating_sub(result.len()) {
            return None;
        }
        result.extend_from_slice(&chunk);
    }
    Some(result)
}

async fn local_health(panel: &Panel, config: &Config) -> Option<Value> {
    let response = panel
        .http
        .get(format!("http://{}/health", config.listen))
        .bearer_auth(&config.api_key)
        .send()
        .await
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let body = limited_body(response, 8192).await?;
    serde_json::from_slice(&body).ok()
}

async fn status(State(panel): State<Panel>) -> Json<Value> {
    let control = panel.control.lock().await;
    let mut result = json!({"version": env!("CARGO_PKG_VERSION"), "configured": control.config.is_some(),
        "running": control.active.is_some(), "phase": "not_configured", "relay": "disabled",
        "joining": control.joining, "default_relay": crate::mailbox::couple::DEFAULT_RELAY});
    if let Some(config) = &control.config {
        result["api_base"] = json!(format!("http://{}/v1", config.listen));
        result["transport"] = json!(config.transport);
        result["phase"] = json!("disconnected");
        if let Some(active) = &control.active {
            result["phase"] = json!("connecting");
            if active.task.is_finished() {
                result["phase"] = json!("stopped");
            } else if let Some(health) = local_health(&panel, config).await {
                result["phase"] = json!(if health["connected"] == true {
                    "connected"
                } else if health["recovering"] == true {
                    "recovering"
                } else if health["access_denied"] == true {
                    "denied"
                } else if health["pairing_rejected"] == true {
                    "rejected"
                } else if health["new_code_required"] == true {
                    "new_code_required"
                } else {
                    "connecting"
                });
            }
        }
    }
    if control.joining {
        result["phase"] = json!("joining");
    }
    Json(result)
}

async fn connect(State(panel): State<Panel>) -> Response {
    let mut control = panel.control.lock().await;
    if control.active.is_some() {
        return failure(
            StatusCode::CONFLICT,
            "The client is already running. Use Disconnect before starting another tunnel.",
        );
    }
    let Some(config) = control.config.clone() else {
        return failure(
            StatusCode::BAD_REQUEST,
            "Import your private client.toml first.",
        );
    };
    let listener = match TcpListener::bind(config.listen).await {
        Ok(listener) => listener,
        Err(_) => {
            return failure(
                StatusCode::CONFLICT,
                "The local API port is occupied. Stop the other client or import a configuration with another loopback port.",
            );
        }
    };
    let state_dir = panel.directory.join("pairing-state");
    let gateway = match Gateway::new(config) {
        Ok(gateway) => gateway,
        Err(_) => {
            return failure(
                StatusCode::BAD_REQUEST,
                "The client configuration cannot be started.",
            );
        }
    };
    let stop = panel.stop.child_token();
    let shutdown = stop.clone();
    let task = tokio::spawn(async move {
        let background = gateway.clone();
        let token = shutdown.clone();
        let peer = tokio::spawn(async move {
            background.maintain(token, state_dir).await;
        });
        let server = axum::serve(listener, gateway.router())
            .with_graceful_shutdown(shutdown.clone().cancelled_owned());
        let _ = shutdown::with_grace(&shutdown, shutdown::GRACE, server.into_future()).await;
        shutdown.cancel();
        let _ = peer.await;
    });
    control.active = Some(Active { stop, task });
    Json(json!({"started": true})).into_response()
}

async fn stop_client(panel: &Panel) {
    let mut control = panel.control.lock().await;
    if let Some(active) = control.active.take() {
        active.stop.cancel();
        let _ = active.task.await;
    }
}

async fn disconnect(State(panel): State<Panel>) -> Json<Value> {
    stop_client(&panel).await;
    Json(json!({"stopped": true}))
}

async fn credentials(State(panel): State<Panel>) -> Response {
    let control = panel.control.lock().await;
    match &control.config {
        Some(config) => Json(
            json!({"api_base": format!("http://{}/v1", config.listen), "api_key": config.api_key}),
        )
        .into_response(),
        None => failure(
            StatusCode::BAD_REQUEST,
            "Import a client configuration first.",
        ),
    }
}

async fn test_backend(State(panel): State<Panel>) -> Response {
    let control = panel.control.lock().await;
    let Some(config) = control.config.as_ref().filter(|_| control.active.is_some()) else {
        return failure(StatusCode::CONFLICT, "Start the connection first.");
    };
    if local_health(&panel, config)
        .await
        .is_none_or(|value| value["connected"] != true)
    {
        return failure(
            StatusCode::CONFLICT,
            "The encrypted tunnel is not connected yet. Check the remote host, pairing code and network permissions.",
        );
    }
    let start = Instant::now();
    let response = panel
        .http
        .get(format!("http://{}/v1/models", config.listen))
        .bearer_auth(&config.api_key)
        .send()
        .await;
    let Ok(response) = response else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "The backend did not respond before the test timeout.",
        );
    };
    if response.status() != StatusCode::OK {
        return failure(
            StatusCode::BAD_GATEWAY,
            "The tunnel is connected, but the backend rejected model discovery. Check its loaded model and required privacy profile.",
        );
    }
    let Some(body) = limited_body(response, 65536).await else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "The model-list response was interrupted or exceeded the size limit.",
        );
    };
    let models = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|value| value.get("data").and_then(Value::as_array).cloned());
    let Some(models) = models else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "The backend did not return a valid model list.",
        );
    };
    let ids: Vec<_> = models
        .iter()
        .take(32)
        .filter_map(|model| model["id"].as_str())
        .map(|id| id.chars().take(256).collect::<String>())
        .collect();
    Json(
        json!({"models": ids, "round_trip_ms": start.elapsed().as_secs_f64() * 1000.0,
        "scope": "GET /v1/models through the tunnel; not token latency or pure network RTT"}),
    )
    .into_response()
}

async fn quit(State(panel): State<Panel>) -> Json<Value> {
    panel.stop.cancel();
    Json(json!({"closing": true}))
}

pub fn default_directory() -> anyhow::Result<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    Ok(base
        .ok_or_else(|| Diagnostic::err("cannot find the private application directory"))?
        .join("k-lani-link/control-panel"))
}

fn open_browser(address: &str) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let windows = std::env::var_os("SystemRoot").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Windows shell directory is unavailable",
            )
        })?;
        // Do not search the current project directory or PATH for a shell executable.
        std::process::Command::new(PathBuf::from(windows).join("explorer.exe"))
    };
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("/usr/bin/open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = std::process::Command::new("/usr/bin/xdg-open");
    command
        .arg(address)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

struct Instance {
    lock: File,
    address_file: PathBuf,
}
impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.address_file);
        let _ = self.lock.unlock();
    }
}

fn saved_address(text: &str) -> Option<String> {
    let url = url::Url::parse(text.trim()).ok()?;
    let token = url.fragment()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || token.len() != 43
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    Some(url.to_string())
}

pub async fn run(
    directory: PathBuf,
    stop: CancellationToken,
    launch_browser: bool,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(
        directory
            .parent()
            .ok_or_else(|| Diagnostic::err("invalid application directory"))?,
    )?;
    private_directory(&directory)?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(directory.join("panel.lock"))?;
    let address_file = directory.join("panel-address.txt");
    if lock.try_lock().is_err() {
        let address = std::fs::read_to_string(&address_file)
            .ok()
            .and_then(|text| saved_address(&text))
            .ok_or_else(|| {
                Diagnostic::err(
                    "the control panel is already starting; try opening it again shortly",
                )
            })?;
        if launch_browser {
            let _ = open_browser(&address).map_err(|_| {
                Diagnostic::err("cannot reopen the control panel in the default browser")
            })?;
        }
        return Ok(());
    }
    let instance = Instance { lock, address_file };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address: SocketAddr = listener.local_addr()?;
    let origin = format!("http://{address}");
    let token = random_secret()?;
    let saved = directory.join("client.toml");
    let config = if let Some(config) = paired(&directory) {
        Some(config)
    } else if saved.is_file() {
        Config::load(&saved)
            .ok()
            .and_then(|cfg| toml::to_string(&cfg).ok())
            .and_then(|text| imported(&text, &directory).ok())
    } else {
        None
    };
    let panel = Panel {
        token,
        origin,
        directory,
        control: Arc::new(Mutex::new(Control {
            config,
            active: None,
            joining: false,
        })),
        stop: stop.clone(),
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()?,
        relay_ca: None,
    };
    // A pairing by code reconnects without a new code, so the panel starts it right away.
    let https = panel
        .control
        .lock()
        .await
        .config
        .as_ref()
        .is_some_and(|config| config.transport == crate::config::Transport::Https);
    if https {
        let _ = connect(State(panel.clone())).await;
    }
    let address = format!("{}/#{}", panel.origin, panel.token);
    if instance.address_file.exists() {
        std::fs::remove_file(&instance.address_file)?;
    }
    private_file(&instance.address_file, &address)?;
    if launch_browser {
        let mut browser = open_browser(&address).map_err(|_| {
            Diagnostic::err(
                "cannot open the default browser; configure a default browser and start again",
            )
        })?;
        // Reap the launcher without logging its output or the private bootstrap URL.
        std::thread::spawn(move || {
            let _ = browser.wait();
        });
    }
    let server = axum::serve(listener, router(panel.clone()))
        .with_graceful_shutdown(stop.clone().cancelled_owned());
    let _ = shutdown::with_grace(&stop, shutdown::GRACE, server.into_future()).await;
    stop.cancel();
    stop_client(&panel).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::config;

    async fn fixture() -> (Panel, JoinHandle<()>, PathBuf) {
        let directory =
            std::env::temp_dir().join(format!("link-panel-{}", random_secret().unwrap()));
        private_directory(&directory).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let panel = Panel {
            token: random_secret().unwrap(),
            origin: format!("http://{}", listener.local_addr().unwrap()),
            directory: directory.clone(),
            control: Default::default(),
            stop: CancellationToken::new(),
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap(),
            relay_ca: None,
        };
        let app = router(panel.clone());
        let stop = panel.stop.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        (panel, task, directory)
    }

    async fn cleanup(panel: Panel, task: JoinHandle<()>, directory: PathBuf) {
        panel.stop.cancel();
        stop_client(&panel).await;
        task.await.unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn panel_requires_token_and_origin_and_never_embeds_keys_in_static_content() {
        let (panel, task, directory) = fixture().await;
        let page = panel.http.get(&panel.origin).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(page.headers()["cache-control"], "no-store");
        assert!(
            page.headers()["content-security-policy"]
                .to_str()
                .unwrap()
                .contains("frame-ancestors 'none'")
        );
        assert!(!page.text().await.unwrap().contains(&panel.token));
        for path in [
            "status",
            "config",
            "connect",
            "disconnect",
            "credentials",
            "test",
            "quit",
        ] {
            let request = if path == "status" {
                panel.http.get(format!("{}/api/{path}", panel.origin))
            } else {
                panel
                    .http
                    .post(format!("{}/api/{path}", panel.origin))
                    .json(&json!({}))
            };
            assert_eq!(
                request.send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let bad_origin = panel
            .http
            .post(format!("{}/api/connect", panel.origin))
            .bearer_auth(&panel.token)
            .header("origin", "https://untrusted.example.invalid")
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
        let bad_host = panel
            .http
            .get(format!("{}/api/status", panel.origin))
            .bearer_auth(&panel.token)
            .header("host", "untrusted.example.invalid")
            .send()
            .await
            .unwrap();
        assert_eq!(bad_host.status(), StatusCode::FORBIDDEN);
        cleanup(panel, task, directory).await;
    }

    #[tokio::test]
    async fn import_normalizes_paths_preserves_markers_and_reports_errors_without_echoing_input() {
        let (panel, task, directory) = fixture().await;
        let state = directory.join("pairing-state");
        private_directory(&state).unwrap();
        std::fs::write(state.join("existing.used"), b"").unwrap();
        let mut cfg = config();
        cfg.pairing_state_dir = Some(PathBuf::from("/not/our/state"));
        cfg.management_token_file = Some(PathBuf::from("/private/operator-key"));
        let text = toml::to_string(&cfg).unwrap();
        let response = panel
            .http
            .post(format!("{}/api/config", panel.origin))
            .bearer_auth(&panel.token)
            .json(&json!({"toml": text}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let saved = Config::load(&directory.join("client.toml")).unwrap();
        assert_eq!(saved.pairing_state_dir, Some(state.clone()));
        assert!(saved.management_token_file.is_none());
        assert!(state.join("existing.used").exists());
        let status = panel
            .http
            .get(format!("{}/api/status", panel.origin))
            .bearer_auth(&panel.token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!status.contains(&cfg.api_key) && !status.contains(&cfg.pair_key));
        for input in [
            json!({"toml": "secret-unparseable-value = ["}),
            json!({"secret-unparseable-value": "secret-unparseable-value"}),
        ] {
            let response = panel
                .http
                .post(format!("{}/api/config", panel.origin))
                .bearer_auth(&panel.token)
                .json(&input)
                .send()
                .await
                .unwrap();
            assert!(!response.status().is_success());
            assert!(
                !response
                    .text()
                    .await
                    .unwrap()
                    .contains("secret-unparseable-value")
            );
        }
        let response = panel
            .http
            .post(format!("{}/api/config", panel.origin))
            .bearer_auth(&panel.token)
            .json(&json!({"toml": "x".repeat(66000)}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            Config::load(&directory.join("client.toml"))
                .unwrap()
                .pair_key,
            cfg.pair_key
        );
        cleanup(panel, task, directory).await;
    }

    #[tokio::test]
    async fn occupied_api_port_and_replacing_a_running_config_fail_cleanly() {
        let (panel, task, directory) = fixture().await;
        let occupied = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = config();
        cfg.listen = occupied.local_addr().unwrap();
        panel.control.lock().await.config = Some(cfg.clone());
        assert_eq!(
            connect(State(panel.clone())).await.status(),
            StatusCode::CONFLICT
        );
        assert!(panel.control.lock().await.active.is_none());
        drop(occupied);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        assert_eq!(
            connect(State(panel.clone())).await.status(),
            StatusCode::CONFLICT
        );
        let response = save(
            State(panel.clone()),
            Ok(Json(Import {
                toml: toml::to_string(&cfg).unwrap(),
            })),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
        stop_client(&panel).await;
        assert!(panel.control.lock().await.active.is_none());
        // Parallel crash-matrix children can briefly inherit a listening descriptor
        // until exec closes it. Assert bounded release, not the fork/exec instant.
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if TcpListener::bind(cfg.listen).await.is_ok() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cleanup(panel, task, directory).await;
    }

    #[test]
    fn reopening_is_limited_to_a_private_loopback_panel_and_turn_stays_explicitly_off() {
        let token = "a".repeat(43);
        assert!(saved_address(&format!("http://127.0.0.1:12345/#{token}")).is_some());
        for url in [
            format!("https://example.invalid/#{token}"),
            format!("http://localhost:12345/#{token}"),
            format!("http://127.0.0.1:12345/other#{token}"),
            format!("http://user@127.0.0.1:12345/#{token}"),
            format!("http://127.0.0.1:12345/?secret=other#{token}"),
            "http://127.0.0.1:12345/#short".into(),
        ] {
            assert!(saved_address(&url).is_none());
        }
        let mut cfg = config();
        cfg.relay = RelayPolicy::Allowed;
        assert!(
            imported(
                &toml::to_string(&cfg).unwrap(),
                std::path::Path::new("/unused")
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn response_size_limit_is_enforced_before_buffering_the_whole_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/", get(|| async { "x".repeat(70000) })),
            )
            .await
            .unwrap();
        });
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap();
        assert!(limited_body(response, 65536).await.is_none());
        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn panel_connects_tests_a_real_encrypted_tunnel_and_preserves_consumed_state() {
        let (panel, ui_task, directory) = fixture().await;
        let signal_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = config();
        cfg.signaling_url = format!("http://{}", signal_listener.local_addr().unwrap());
        cfg.upstream = format!("http://{}", backend_listener.local_addr().unwrap());
        cfg.listen = api_listener.local_addr().unwrap();
        drop(api_listener);
        let broker = tokio::spawn(
            axum::serve(
                signal_listener,
                crate::signal::router(cfg.signaling_token.clone()),
            )
            .into_future(),
        );
        let backend = Router::new()
            .route(
                "/health",
                get(|| async {
                    Json(json!({"link_privacy": {
                        "version": 1, "application_logs": false, "request_history": false,
                        "diagnostics": false, "remote_images": false
                    }}))
                }),
            )
            .route(
                "/v1/models",
                get(|| async { Json(json!({"data": [{"id": "panel-test-model"}]})) }),
            );
        let backend = tokio::spawn(axum::serve(backend_listener, backend).into_future());
        let remote_stop = CancellationToken::new();
        let host = tokio::spawn(crate::host::run(
            cfg.clone(),
            remote_stop.clone(),
            directory.join("host-state"),
        ));
        panel.control.lock().await.config = Some(cfg);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if status(State(panel.clone())).await.0["phase"] == "connected" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        broker.abort(); // Inference must still work after the intermediary stops.
        let response = test_backend(State(panel.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["models"][0], "panel-test-model");
        assert!(result["round_trip_ms"].is_number());
        stop_client(&panel).await;
        remote_stop.cancel();
        host.await.unwrap().unwrap();
        backend.abort();
        assert!(
            std::fs::read_dir(directory.join("pairing-state"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "used"))
        );
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if status(State(panel.clone())).await.0["phase"] == "new_code_required" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        cleanup(panel, ui_task, directory).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_pairing_code_configures_the_panel_for_the_https_relay_and_connects() {
        let (mut panel, ui_task, directory) = fixture().await;
        let relay = crate::mailbox::couple::tests::fixture().await;
        panel.relay_ca = Some(relay.ca.clone());
        let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_address = backend_listener.local_addr().unwrap();
        let backend = Router::new()
            .route(
                "/health",
                get(|| async {
                    Json(json!({"link_privacy": {
                        "version": 1, "application_logs": false, "request_history": false,
                        "diagnostics": false, "remote_images": false
                    }}))
                }),
            )
            .route(
                "/v1/models",
                get(|| async { Json(json!({"data": [{"id": "relay-test-model"}]})) }),
            );
        let backend = tokio::spawn(axum::serve(backend_listener, backend).into_future());
        let (offer, code) = relay.offer().await;
        let joined = join(
            State(panel.clone()),
            Ok(Json(JoinInput {
                code,
                relay: relay.relay.clone(),
            })),
        )
        .await;
        assert_eq!(joined.status(), StatusCode::OK);
        offer.await.unwrap().unwrap();
        // The pairing survives a restart of the panel.
        let saved = paired(&directory).unwrap();
        assert!(saved.transport == crate::config::Transport::Https);
        let mut host = Config::load(&relay.root.join("pairing/host/host.toml")).unwrap();
        host.upstream = format!("http://{backend_address}");
        let remote_stop = CancellationToken::new();
        let host_task = tokio::spawn(crate::host::run(
            host,
            remote_stop.clone(),
            relay.root.join("host-pairing-state"),
        ));
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        panel.control.lock().await.config.as_mut().unwrap().listen = api.local_addr().unwrap();
        drop(api);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let state = status(State(panel.clone())).await.0;
                if state["phase"] == "connected" {
                    assert_eq!(state["transport"], "https");
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let response = test_backend(State(panel.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["models"][0], "relay-test-model");
        // A second code while connected is refused before anything is contacted.
        let again = join(
            State(panel.clone()),
            Ok(Json(JoinInput {
                code: "472-1234-5678".into(),
                relay: relay.relay.clone(),
            })),
        )
        .await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
        stop_client(&panel).await;
        remote_stop.cancel();
        let _ = host_task.await.unwrap();
        backend.abort();
        cleanup(panel, ui_task, directory).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_relay_that_refuses_the_token_is_shown_as_access_refused() {
        let (panel, ui_task, directory) = fixture().await;
        let relay = crate::mailbox::couple::tests::fixture().await;
        let stranger = directory.join("stranger-token");
        private_file(&stranger, &random_secret().unwrap()).unwrap();
        crate::mailbox::setup::issue(
            &directory.join("issued"),
            crate::mailbox::Options {
                relays: vec![relay.relay.clone()],
                state_dir: directory.join("unused"),
                admission_token_file: stranger,
                proxy_url: None,
                ca_cert_file: Some(relay.ca.clone()),
                poll_wait_seconds: 25,
                http1_only: false,
                http3: false,
                post_writes: false,
            },
            None,
            Default::default(),
            &Default::default(),
        )
        .unwrap();
        let mut config = Config::load(&directory.join("issued/client/client.toml")).unwrap();
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        config.listen = api.local_addr().unwrap();
        drop(api);
        panel.control.lock().await.config = Some(config);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while status(State(panel.clone())).await.0["phase"] != "denied" {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("a 401 from the relay must surface as access refused");
        cleanup(panel, ui_task, directory).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn closing_the_tab_while_pairing_neither_aborts_it_nor_leaves_the_panel_waiting() {
        let (mut panel, ui_task, directory) = fixture().await;
        let relay = crate::mailbox::couple::tests::fixture().await;
        panel.relay_ca = Some(relay.ca.clone());
        let (offer, code) = relay.offer().await;
        let request = tokio::spawn(join(
            State(panel.clone()),
            Ok(Json(JoinInput {
                code,
                relay: relay.relay.clone(),
            })),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        request.abort();
        offer.await.unwrap().unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let state = status(State(panel.clone())).await.0;
                if state["joining"] == false && state["configured"] == true {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the pairing finishes and the panel stops waiting");
        cleanup(panel, ui_task, directory).await;
    }
}
