//! Local browser control panel. No telemetry or request logging; a chat is stored only when
//! the user saves it, in the panel's private directory (see `chat`).
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
use sha2::Digest;
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
const CHAT_JS: &str = include_str!("ui/chat.js");
const SCAN_JS: &str = include_str!("ui/scan.js");
const CSS: &str = include_str!("ui/style.css");

struct Active {
    stop: CancellationToken,
    task: JoinHandle<()>,
}
/// One model computer the panel connects to.
struct Connection {
    /// Its pairing folder inside the panel directory, or `imported` for a client.toml.
    id: String,
    config: Config,
    /// Its tunnel while connected.
    active: Option<Active>,
    gateway: Option<Gateway>,
}
impl Connection {
    fn new(id: String, config: Config) -> Self {
        Self {
            id,
            config,
            active: None,
            gateway: None,
        }
    }
    /// The last day of a free trial on its relay (UTC), read from its trial token.
    fn trial_until(&self) -> Option<String> {
        let path = &self.config.https.as_ref()?.admission_token_file;
        let mut token = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(path).ok()?, 512),
            &mut token,
        )
        .ok()?;
        let expires = crate::mailbox::trial::expiry(&token)?;
        Some(crate::mailbox::trial::day(expires.saturating_sub(1)))
    }
    /// Where its relay or signaling server is, for telling computers apart in the panel.
    fn label(&self) -> String {
        let address = self
            .config
            .https
            .as_ref()
            .and_then(|https| https.relays.first().cloned())
            .unwrap_or_else(|| self.config.signaling_url.clone());
        url::Url::parse(&address)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or(address)
    }
}
#[derive(Default)]
struct Control {
    /// The first one's local address is the front's; see `front`.
    connections: Vec<Connection>,
    /// The one local API in front of all connections.
    front: Option<Active>,
    /// A pairing by code is waiting for the other computer.
    joining: bool,
}
const IMPORTED: &str = "imported";
#[derive(Clone)]
struct Panel {
    token: String,
    origin: String,
    directory: PathBuf,
    control: Arc<Mutex<Control>>,
    stop: CancellationToken,
    http: reqwest::Client,
    /// For chat answers, which stream for as long as the model writes.
    stream: reqwest::Client,
    /// A private CA for the relay; only tests set one, a real relay needs a public certificate.
    relay_ca: Option<PathBuf>,
    updates: Arc<std::sync::Mutex<Updates>>,
    /// What the front reaches, readable without the control lock.
    routes: front::Routes,
    catalog: front::Catalogs,
    /// The front's API key: the first pairing's, kept in the panel folder so that an AI
    /// app keeps working when computers are added or removed.
    key: Arc<std::sync::RwLock<Option<String>>>,
}
impl Panel {
    fn key(&self) -> Option<String> {
        self.key.read().unwrap().clone()
    }
    /// Loads the front's key, or takes the first connection's when there is none yet.
    fn ensure_key(&self, control: &Control) {
        if self.key().is_some() {
            return;
        }
        let file = self.directory.join(API_KEY_FILE);
        let key = std::fs::read_to_string(&file)
            .ok()
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty())
            .or_else(|| {
                let key = control.connections.first()?.config.api_key.clone();
                private_file(&file, &key).ok()?;
                Some(key)
            });
        *self.key.write().unwrap() = key;
    }
    /// Tells the front which tunnels are running.
    fn publish(&self, control: &Control) {
        let routes = control
            .connections
            .iter()
            .filter(|connection| connection.active.is_some())
            .filter_map(|connection| {
                Some(front::Route {
                    id: connection.id.clone(),
                    key: connection.config.api_key.clone(),
                    gateway: connection.gateway.clone()?,
                })
            })
            .collect();
        *self.routes.write().unwrap() = routes;
    }
}
const API_KEY_FILE: &str = "api-key";
/// What the panel knows about updates; see `crate::update`.
#[derive(Default)]
struct Updates {
    /// Looking for updates by itself is on (the default) or switched off in the panel.
    enabled: bool,
    /// A newer release, and whether it is downloaded, verified and waiting.
    available: Option<String>,
    staged: bool,
    /// This program's folder takes the new file.
    writable: bool,
    /// The user asked to install it now: the panel ends, then the update is installed.
    install: bool,
    /// Wakes the checker after the setting was switched on.
    wake: Arc<tokio::sync::Notify>,
}
const UPDATES_OFF: &str = "updates-off";
/// The page language chosen in the panel. The page's own storage is lost at every start,
/// because each start serves the page from another port.
const LANGUAGE: &str = "language";
const LANGUAGES: [&str; 2] = ["de", "en"];
const STAGE: &str = "update";
/// The most model computers one panel connects to at once.
const MAX_COMPUTERS: usize = 8;

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
            "/chat.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    CHAT_JS,
                )
            }),
        )
        .route(
            "/scan.js",
            get(|| async {
                (
                    [("content-type", "text/javascript; charset=utf-8")],
                    SCAN_JS,
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async { ([("content-type", "text/css; charset=utf-8")], CSS) }),
        )
        .route("/icon/{light}", get(icon))
        .route("/api/status", get(status))
        .route("/api/config", post(save))
        .route("/api/join", post(join))
        .route(
            "/api/scan",
            post(scan).layer(DefaultBodyLimit::max(
                8 + (crate::qr::MAX_SIDE * crate::qr::MAX_SIDE) as usize,
            )),
        )
        .route("/api/connect", post(connect))
        .route("/api/disconnect", post(disconnect))
        .route("/api/remove", post(remove))
        .route("/api/test", post(test_backend))
        .route("/api/credentials", post(credentials))
        .route("/api/quit", post(quit))
        .route("/api/update", post(update))
        .route("/api/language", post(choose_language))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .merge(chat::routes())
        .layer(middleware::from_fn_with_state(panel.clone(), protect))
        .with_state(panel)
}

/// The page's icon: the same square as the notification-area icon, in the state colour.
async fn icon(axum::extract::Path(light): axum::extract::Path<String>) -> Response {
    let light = match light.as_str() {
        "connected.png" => tray::Light::Connected,
        "waiting.png" => tray::Light::Waiting,
        "problem.png" => tray::Light::Problem,
        _ => tray::Light::Idle,
    };
    ([("content-type", "image/png")], tray::png(64, light)).into_response()
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
    config.load_on_demand = false;
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
    if control.front.is_some() {
        return failure(
            StatusCode::CONFLICT,
            "Disconnect before replacing the configuration. A new tunnel needs a fresh pairing code.",
        );
    }
    let Ok(config) = imported(&input.toml, &panel.directory) else {
        return failure(
            StatusCode::BAD_REQUEST,
            "Configuration rejected. Import a fresh client.toml with a valid pairing/API key, loopback API address, HTTPS signaling and TURN disabled.",
        );
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
    // An imported file replaces the pairings by code as the configuration; their folders stay.
    let _ = std::fs::remove_file(panel.directory.join(PAIRING_POINTER));
    let _ = std::fs::remove_file(panel.directory.join(PAIRINGS));
    control.connections = vec![Connection::new(IMPORTED.into(), config)];
    panel.ensure_key(&control);
    Json(json!({"saved": true})).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinInput {
    code: String,
    #[serde(default)]
    relay: String,
}
/// Names the first HTTPS pairing's folder inside the panel directory; older versions read
/// only this one.
const PAIRING_POINTER: &str = "https-pairing.txt";
/// Names every HTTPS pairing's folder, one per line, the first one first.
const PAIRINGS: &str = "https-pairings.txt";

fn valid_folder(name: &str) -> bool {
    name.len() > 6 && name.starts_with("https-") && name[6..].bytes().all(|b| b.is_ascii_hexdigit())
}
/// One HTTPS pairing's configuration from its folder.
fn pairing(directory: &std::path::Path, name: &str) -> Option<Config> {
    if !valid_folder(name) {
        return None;
    }
    Config::load(&directory.join(name).join("client.toml"))
        .ok()
        .filter(|config| config.transport == crate::config::Transport::Https)
}
/// The HTTPS pairings the panel uses, from its list file or, written by an older version,
/// its pointer file.
fn paired(directory: &std::path::Path) -> Vec<Connection> {
    let names = std::fs::read_to_string(directory.join(PAIRINGS))
        .or_else(|_| std::fs::read_to_string(directory.join(PAIRING_POINTER)))
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    names
        .lines()
        .map(str::trim)
        .filter(|name| seen.insert(name.to_owned()))
        .filter_map(|name| Some(Connection::new(name.into(), pairing(directory, name)?)))
        .collect()
}
/// Writes the list and the first pairing's pointer, each replaced whole.
fn save_pairings(directory: &std::path::Path, connections: &[Connection]) -> anyhow::Result<()> {
    let names: Vec<&str> = connections
        .iter()
        .map(|connection| connection.id.as_str())
        .filter(|id| valid_folder(id))
        .collect();
    for (file, text) in [
        (PAIRINGS, names.join("\n")),
        (
            PAIRING_POINTER,
            names.first().copied().unwrap_or_default().to_owned(),
        ),
    ] {
        let target = directory.join(file);
        if text.is_empty() {
            let _ = std::fs::remove_file(&target);
            continue;
        }
        let temporary = directory.join(format!("{file}-{}.tmp", random_secret()?));
        private_file(&temporary, &text)?;
        std::fs::rename(&temporary, &target)?;
    }
    Ok(())
}

/// Pairing by code: receives a client folder from the host and makes it the configuration.
async fn join(
    State(panel): State<Panel>,
    input: Result<Json<JoinInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return failure(StatusCode::BAD_REQUEST, "Enter the pairing code.");
    };
    let Ok(code) = crate::mailbox::couple::Code::parse(&input.code) else {
        return failure(
            StatusCode::BAD_REQUEST,
            "A pairing code has eleven digits, e.g. 472-1234-5678.",
        );
    };
    let relay = match input.relay.trim() {
        "" => match crate::mailbox::couple::DEFAULT_RELAY {
            Some(relay) => relay.to_owned(),
            None => return failure(StatusCode::BAD_REQUEST, "Enter the relay address."),
        },
        relay => relay.to_owned(),
    };
    let local = panel.relay_ca.clone();
    pair(&panel, move |dir| async move {
        crate::mailbox::couple::join(
            &dir,
            &code,
            crate::mailbox::couple::Join {
                relay: &relay,
                proxy_url: None,
                ca_cert_file: local,
            },
        )
        .await
    })
    .await
}

/// Adds a new pairing to the configuration: `receive` writes the client folder it is given.
/// Each pairing gets a new folder; the other computers stay connected.
async fn pair<F, W>(panel: &Panel, receive: W) -> Response
where
    W: FnOnce(PathBuf) -> F + Send + 'static,
    F: std::future::Future<Output = anyhow::Result<PathBuf>> + Send,
{
    {
        let mut control = panel.control.lock().await;
        if control.connections.len() >= MAX_COMPUTERS {
            return failure(
                StatusCode::CONFLICT,
                "Eight model computers are the most. Remove one before adding another.",
            );
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
            let config = receive(worker.directory.join(&name)).await?;
            let config = Config::load(&config)?;
            anyhow::Ok(Connection::new(name, config))
        }
        .await;
        let mut control = worker.control.lock().await;
        control.joining = false;
        match result {
            Ok(connection) => {
                // A pairing by code replaces an imported client.toml, as it always did.
                if control.connections.iter().any(|c| c.id == IMPORTED) && control.front.is_none() {
                    control.connections.retain(|c| c.id != IMPORTED);
                }
                control.connections.push(connection);
                save_pairings(&worker.directory, &control.connections)
                    .map_err(|_| "Cannot save the pairing.".to_owned())?;
                worker.ensure_key(&control);
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

/// Pairing by QR code: one greyscale camera frame (width and height as little-endian u32,
/// then one byte per pixel). Without a code in it the answer is `found: false`; a
/// k-lani-link code pairs at once. The code's content never goes back to the page.
async fn scan(State(panel): State<Panel>, body: axum::body::Bytes) -> Response {
    let size = |at: usize| {
        body.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (Some(width), Some(height)) = (size(0), size(4)) else {
        return failure(StatusCode::BAD_REQUEST, "This is not a camera picture.");
    };
    let fits = (1..=crate::qr::MAX_SIDE).contains(&width)
        && (1..=crate::qr::MAX_SIDE).contains(&height)
        && body.len() - 8 == width as usize * height as usize;
    if !fits {
        return failure(StatusCode::BAD_REQUEST, "This is not a camera picture.");
    }
    let pixels = body.slice(8..).to_vec();
    let found = tokio::task::spawn_blocking(move || crate::qr::read(width, height, pixels))
        .await
        .ok()
        .flatten();
    let Some(text) = found else {
        return Json(json!({"found": false})).into_response();
    };
    if !crate::mailbox::couple::is_qr_pairing(&text) {
        return failure(
            StatusCode::BAD_REQUEST,
            "This QR code is not a k-lani-link pairing code. Show the code from offer-https --qr.",
        );
    }
    // A QR code pairs once; its secret is void after the first connection anyway.
    let used = panel.directory.join(QR_USED);
    let fingerprint: String = sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let seen = std::fs::read_to_string(&used).unwrap_or_default();
    if seen.lines().any(|line| line == fingerprint) {
        return failure(
            StatusCode::CONFLICT,
            "This QR code was already used. Show a new one with offer-https --qr.",
        );
    }
    let local = panel.relay_ca.clone();
    let response = pair(&panel, move |dir| async move {
        crate::mailbox::couple::join_qr(&dir, &text, None, local)
    })
    .await;
    if response.status() == StatusCode::OK {
        let _ = std::fs::write(&used, format!("{seen}{fingerprint}\n"));
    }
    response
}
/// Fingerprints of the QR codes this panel paired with.
const QR_USED: &str = "qr-used.txt";

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

async fn status(State(panel): State<Panel>) -> Json<Value> {
    Json(snapshot(&panel).await)
}

/// One connection's phase, from its own gateway's health.
async fn phase_of(panel: &Panel, connection: &Connection) -> &'static str {
    let Some(active) = &connection.active else {
        return "disconnected";
    };
    // A tunnel that gave up (access refused, code used) still reports why.
    let ended = active.task.is_finished();
    let route = panel
        .routes
        .read()
        .unwrap()
        .iter()
        .find(|route| route.id == connection.id)
        .cloned();
    let Some(health) = (match &route {
        Some(route) => front::health_of(route).await,
        None => None,
    }) else {
        return if ended { "stopped" } else { "connecting" };
    };
    if health["access_denied"] == true {
        "denied"
    } else if health["pairing_rejected"] == true {
        "rejected"
    } else if health["new_code_required"] == true {
        "new_code_required"
    } else if ended {
        "stopped"
    } else if health["connected"] == true {
        "connected"
    } else if health["recovering"] == true {
        "recovering"
    } else if health["relay_busy"] == true {
        // Waiting does not help here; another connection must end first.
        "relay_busy"
    } else {
        "connecting"
    }
}

/// The panel's state, for the page and the notification-area icon. With several model
/// computers, `phase` is `connected` while at least one is; each has its own entry in
/// `computers`.
async fn snapshot(panel: &Panel) -> Value {
    let control = panel.control.lock().await;
    let mut result = json!({"version": env!("CARGO_PKG_VERSION"),
        "configured": !control.connections.is_empty(), "running": control.front.is_some(),
        "phase": "not_configured", "relay": "disabled", "joining": control.joining,
        "default_relay": crate::mailbox::couple::DEFAULT_RELAY,
        "tray": cfg!(any(windows, target_os = "macos"))});
    {
        let updates = panel.updates.lock().unwrap();
        result["update"] = json!({
            "supported": crate::update::BUILT_IN && crate::update::PLATFORM.is_some(),
            "enabled": updates.enabled, "available": updates.available,
            "staged": updates.staged, "writable": updates.writable,
        });
    }
    if let Ok(chosen) = std::fs::read_to_string(panel.directory.join(LANGUAGE))
        && LANGUAGES.contains(&chosen.trim())
    {
        result["language"] = json!(chosen.trim());
    }
    if let Some(first) = control.connections.first() {
        result["api_base"] = json!(format!("http://{}/v1", first.config.listen));
        let https = control
            .connections
            .iter()
            .all(|c| c.config.transport == crate::config::Transport::Https);
        result["transport"] = if https {
            json!(crate::config::Transport::Https)
        } else {
            json!(first.config.transport)
        };
        let mut computers = Vec::new();
        for connection in &control.connections {
            let phase = phase_of(panel, connection).await;
            computers.push(
                json!({"id": connection.id, "label": connection.label(), "phase": phase,
                "models": front::listed_by(&panel.catalog, &connection.id),
                "trial_until": connection.trial_until()}),
            );
        }
        let front_stopped = control
            .front
            .as_ref()
            .is_some_and(|front| front.task.is_finished());
        result["phase"] = if control.front.is_none() {
            json!("disconnected")
        } else if front_stopped {
            json!("stopped")
        } else if computers.iter().any(|c| c["phase"] == "connected") {
            json!("connected")
        } else {
            computers[0]["phase"].clone()
        };
        result["computers"] = json!(computers);
    }
    if control.joining {
        result["phase"] = json!("joining");
    }
    result
}

/// Starts the front and every connection that is not running yet: the panel's own start,
/// "Connect", and a computer added while the others are connected.
async fn connect(State(panel): State<Panel>) -> Response {
    let mut control = panel.control.lock().await;
    let Some(listen) = control.connections.first().map(|c| c.config.listen) else {
        return failure(
            StatusCode::BAD_REQUEST,
            "Import your private client.toml first.",
        );
    };
    if control.front.is_some() && control.connections.iter().all(|c| c.active.is_some()) {
        return failure(
            StatusCode::CONFLICT,
            "The client is already running. Use Disconnect before starting another tunnel.",
        );
    }
    let mut started = Vec::new();
    for connection in control.connections.iter().filter(|c| c.active.is_none()) {
        let mut config = connection.config.clone();
        // The host decides how many requests run at once; this side only must not be the
        // narrower door.
        config.max_concurrent_requests = config.max_concurrent_requests.max(8);
        config.max_queued_requests = config.max_queued_requests.max(16);
        let Ok(gateway) = Gateway::new(config) else {
            return failure(
                StatusCode::BAD_REQUEST,
                "The client configuration cannot be started.",
            );
        };
        started.push((connection.id.clone(), gateway));
    }
    if control.front.is_none() {
        let Ok(listener) = TcpListener::bind(listen).await else {
            return failure(
                StatusCode::CONFLICT,
                "The local API port is occupied. Stop the other client or import a configuration with another loopback port.",
            );
        };
        panel.ensure_key(&control);
        let stop = panel.stop.child_token();
        let shutdown = stop.clone();
        let app = front::router(panel.clone());
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, app)
                .with_graceful_shutdown(shutdown.clone().cancelled_owned());
            let _ = shutdown::with_grace(&shutdown, shutdown::GRACE, server.into_future()).await;
            shutdown.cancel();
        });
        control.front = Some(Active { stop, task });
    }
    let state_dir = panel.directory.join("pairing-state");
    for (id, gateway) in started {
        let stop = panel.stop.child_token();
        let (background, token, state_dir) = (gateway.clone(), stop.clone(), state_dir.clone());
        let task = tokio::spawn(async move { background.maintain(token, state_dir).await });
        if let Some(connection) = control.connections.iter_mut().find(|c| c.id == id) {
            connection.active = Some(Active { stop, task });
            connection.gateway = Some(gateway);
        }
    }
    panel.publish(&control);
    Json(json!({"started": true})).into_response()
}

async fn stop_connection(connection: &mut Connection) {
    connection.gateway = None;
    if let Some(active) = connection.active.take() {
        active.stop.cancel();
        let _ = active.task.await;
    }
}

async fn stop_client(panel: &Panel) {
    let mut control = panel.control.lock().await;
    if let Some(front) = control.front.take() {
        front.stop.cancel();
        let _ = front.task.await;
    }
    for connection in &mut control.connections {
        stop_connection(connection).await;
    }
    panel.publish(&control);
    *panel.catalog.lock().unwrap() = front::Catalog::default();
}

async fn disconnect(State(panel): State<Panel>) -> Json<Value> {
    stop_client(&panel).await;
    Json(json!({"stopped": true}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveInput {
    id: String,
}
/// Ends one model computer's connection and deletes its pairing; the others keep running.
async fn remove(
    State(panel): State<Panel>,
    input: Result<Json<RemoveInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return failure(StatusCode::BAD_REQUEST, "Unknown model computer.");
    };
    let mut control = panel.control.lock().await;
    let Some(index) = control.connections.iter().position(|c| c.id == input.id) else {
        return failure(StatusCode::NOT_FOUND, "Unknown model computer.");
    };
    let mut connection = control.connections.remove(index);
    stop_connection(&mut connection).await;
    if control.connections.is_empty()
        && let Some(front) = control.front.take()
    {
        front.stop.cancel();
        let _ = front.task.await;
    }
    panel.publish(&control);
    *panel.catalog.lock().unwrap() = front::Catalog::default();
    if save_pairings(&panel.directory, &control.connections).is_err() {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Cannot save the pairing list.",
        );
    }
    let gone = if valid_folder(&connection.id) {
        std::fs::remove_dir_all(panel.directory.join(&connection.id))
    } else {
        std::fs::remove_file(panel.directory.join("client.toml"))
    };
    if gone.is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound) {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The pairing was removed, but its folder could not be deleted.",
        );
    }
    Json(json!({"removed": true})).into_response()
}

async fn credentials(State(panel): State<Panel>) -> Response {
    let control = panel.control.lock().await;
    panel.ensure_key(&control);
    match (control.connections.first(), panel.key()) {
        (Some(first), Some(key)) => {
            Json(json!({"api_base": format!("http://{}/v1", first.config.listen), "api_key": key}))
                .into_response()
        }
        _ => failure(
            StatusCode::BAD_REQUEST,
            "Import a client configuration first.",
        ),
    }
}

async fn test_backend(State(panel): State<Panel>) -> Response {
    let listen = {
        let control = panel.control.lock().await;
        let Some(first) = control
            .connections
            .first()
            .filter(|_| control.front.is_some())
        else {
            return failure(StatusCode::CONFLICT, "Start the connection first.");
        };
        first.config.listen
    };
    let Some(key) = panel.key() else {
        return failure(StatusCode::CONFLICT, "Start the connection first.");
    };
    let routes = panel.routes.read().unwrap().clone();
    let mut connected = false;
    for route in &routes {
        connected |= front::health_of(route)
            .await
            .is_some_and(|health| health["connected"] == true);
    }
    if !connected {
        return failure(
            StatusCode::CONFLICT,
            "The encrypted tunnel is not connected yet. Check the remote host, pairing code and network permissions.",
        );
    }
    let start = Instant::now();
    let response = panel
        .http
        .get(format!("http://{listen}/v1/models"))
        .bearer_auth(&key)
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
    let Some(body) = limited_body(response, 1024 * 1024).await else {
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
        .take(64)
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

/// Runs the panel until it is quit. True: the user asked to install a downloaded update.
pub async fn run(
    directory: PathBuf,
    stop: CancellationToken,
    launch_browser: bool,
) -> anyhow::Result<bool> {
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
        return Ok(false);
    }
    let instance = Instance { lock, address_file };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address: SocketAddr = listener.local_addr()?;
    let origin = format!("http://{address}");
    let token = random_secret()?;
    let saved = directory.join("client.toml");
    let mut connections = paired(&directory);
    if connections.is_empty()
        && saved.is_file()
        && let Some(config) = Config::load(&saved)
            .ok()
            .and_then(|cfg| toml::to_string(&cfg).ok())
            .and_then(|text| imported(&text, &directory).ok())
    {
        connections.push(Connection::new(IMPORTED.into(), config));
    }
    let updates_off = directory_has(&directory, UPDATES_OFF);
    let panel = Panel {
        token,
        origin,
        directory,
        control: Arc::new(Mutex::new(Control {
            connections,
            ..Control::default()
        })),
        stop: stop.clone(),
        http: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(5))
            .build()?,
        stream: streaming_client()?,
        relay_ca: None,
        updates: Arc::new(std::sync::Mutex::new(Updates {
            enabled: !updates_off,
            writable: std::env::current_exe().is_ok_and(|exe| crate::update::writable(&exe)),
            ..Updates::default()
        })),
        routes: Default::default(),
        catalog: Default::default(),
        key: Default::default(),
    };
    // A pairing by code reconnects without a new code, so the panel starts it right away.
    let https = {
        let control = panel.control.lock().await;
        panel.ensure_key(&control);
        !control.connections.is_empty()
            && control
                .connections
                .iter()
                .all(|c| c.config.transport == crate::config::Transport::Https)
    };
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
    #[cfg(any(windows, target_os = "macos"))]
    let tray = tray::start(panel.clone(), address.clone());
    let checker = tokio::spawn(check_updates(panel.clone()));
    let server = axum::serve(listener, router(panel.clone()))
        .with_graceful_shutdown(stop.clone().cancelled_owned());
    let _ = shutdown::with_grace(&stop, shutdown::GRACE, server.into_future()).await;
    stop.cancel();
    checker.abort();
    stop_client(&panel).await;
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(tray) = tray {
        tray.close().await;
    }
    let install = panel.updates.lock().unwrap().install;
    Ok(install)
}

fn directory_has(directory: &std::path::Path, name: &str) -> bool {
    directory.join(name).exists()
}

/// Looks for a signed update shortly after the start and then twice a day, while the
/// setting is on. A found update is downloaded and verified, then waits for the next start.
/// Failures (offline, a proxy, GitHub's rate limit) are quiet: the next round tries again.
async fn check_updates(panel: Panel) {
    if !crate::update::BUILT_IN || crate::update::PLATFORM.is_none() {
        return;
    }
    let wake = panel.updates.lock().unwrap().wake.clone();
    tokio::time::sleep(std::time::Duration::from_secs(20)).await;
    loop {
        let (enabled, writable) = {
            let updates = panel.updates.lock().unwrap();
            (updates.enabled, updates.writable)
        };
        if enabled && let Ok(http) = crate::update::client() {
            let stage = panel.directory.join(STAGE);
            if let Ok(Some(release)) = crate::update::latest(&http).await {
                let version = release.version.to_string();
                let ready = crate::update::staged(&stage)
                    .is_some_and(|staged| staged.version == release.version);
                let staged = ready
                    || writable
                        && crate::update::download(&http, &release, &stage)
                            .await
                            .is_ok();
                let mut updates = panel.updates.lock().unwrap();
                updates.available = Some(version);
                updates.staged = staged;
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(12 * 3600)) => {}
            _ = wake.notified() => {}
        }
    }
}

/// Installs an update an earlier run downloaded, unless another panel is running from the
/// same folder. Returns the executable to start instead of this one.
pub fn install_staged(directory: &std::path::Path) -> Option<PathBuf> {
    let stage = directory.join(STAGE);
    let staged = crate::update::staged(&stage)?;
    let lock = File::open(directory.join("panel.lock")).ok();
    if lock.as_ref().is_some_and(|lock| lock.try_lock().is_err()) {
        return None;
    }
    drop(lock);
    let exe = std::env::current_exe().ok()?;
    let result = crate::update::install(&staged, &exe);
    // Installed or not, the same download is not tried again at every start.
    let _ = std::fs::remove_dir_all(&stage);
    result.ok().map(|()| exe)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageInput {
    language: String,
}
async fn choose_language(
    State(panel): State<Panel>,
    input: Result<Json<LanguageInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return failure(StatusCode::BAD_REQUEST, "Unknown language.");
    };
    if !LANGUAGES.contains(&input.language.as_str()) {
        return failure(StatusCode::BAD_REQUEST, "Unknown language.");
    }
    let file = panel.directory.join(LANGUAGE);
    // A new private file replaces the old one whole.
    let written = random_secret().and_then(|suffix| {
        let partial = file.with_extension(format!("{suffix}.part"));
        private_file(&partial, &format!("{}\n", input.language))?;
        std::fs::rename(&partial, &file)?;
        Ok(())
    });
    if written.is_err() {
        return failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The setting could not be saved.",
        );
    }
    Json(json!({"language": input.language})).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInput {
    action: String,
}
/// `install`: end the panel and install the waiting update. `on` / `off`: the setting.
async fn update(
    State(panel): State<Panel>,
    input: Result<Json<UpdateInput>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return failure(StatusCode::BAD_REQUEST, "Unknown update action.");
    };
    let marker = panel.directory.join(UPDATES_OFF);
    let mut updates = panel.updates.lock().unwrap();
    match input.action.as_str() {
        "install" if updates.staged => {
            updates.install = true;
            panel.stop.cancel();
            Json(json!({"restarting": true})).into_response()
        }
        "install" => failure(StatusCode::CONFLICT, "No update is ready yet."),
        "on" => {
            let _ = std::fs::remove_file(&marker);
            updates.enabled = true;
            updates.wake.notify_one();
            Json(json!({"enabled": true})).into_response()
        }
        "off" => {
            if private_file(
                &marker,
                "Looking for updates was switched off in the panel.\n",
            )
            .is_err()
                && !marker.exists()
            {
                return failure(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The setting could not be saved.",
                );
            }
            updates.enabled = false;
            Json(json!({"enabled": false})).into_response()
        }
        _ => failure(StatusCode::BAD_REQUEST, "Unknown update action."),
    }
}

mod chat;
mod front;
mod tray;

/// No total timeout: a chat answer streams for as long as the model writes.
fn streaming_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
}

/// macOS: runs `work` (the panel) beside the menu-bar item; see `tray`.
#[cfg(target_os = "macos")]
pub fn with_menu_bar(work: impl FnOnce() -> bool + Send + 'static) -> bool {
    tray::main_thread(work)
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
            stream: streaming_client().unwrap(),
            relay_ca: None,
            updates: Default::default(),
            routes: Default::default(),
            catalog: Default::default(),
            key: Default::default(),
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
        panel.control.lock().await.connections =
            vec![Connection::new(IMPORTED.into(), cfg.clone())];
        assert_eq!(
            connect(State(panel.clone())).await.status(),
            StatusCode::CONFLICT
        );
        assert!(panel.control.lock().await.front.is_none());
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
        assert!(panel.control.lock().await.front.is_none());
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
        panel.control.lock().await.connections = vec![Connection::new(IMPORTED.into(), cfg)];
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let state = status(State(panel.clone())).await.0;
                if state["phase"] == "connected" {
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
        let saved = paired(&directory);
        assert!(saved.len() == 1 && saved[0].config.transport == crate::config::Transport::Https);
        let mut host = Config::load(&relay.root.join("pairing/host/host.toml")).unwrap();
        host.upstream = format!("http://{backend_address}");
        let remote_stop = CancellationToken::new();
        let host_task = tokio::spawn(crate::host::run(
            host,
            remote_stop.clone(),
            relay.root.join("host-pairing-state"),
        ));
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        panel.control.lock().await.connections[0].config.listen = api.local_addr().unwrap();
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
        // A second, wrong code while connected fails and leaves the connected computer alone.
        let again = join(
            State(panel.clone()),
            Ok(Json(JoinInput {
                code: "472-1234-5678".into(),
                relay: relay.relay.clone(),
            })),
        )
        .await;
        assert_eq!(again.status(), StatusCode::BAD_REQUEST);
        assert_eq!(panel.control.lock().await.connections.len(), 1);
        assert_eq!(status(State(panel.clone())).await.0["phase"], "connected");
        stop_client(&panel).await;
        remote_stop.cancel();
        let _ = host_task.await.unwrap();
        backend.abort();
        cleanup(panel, ui_task, directory).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_scanned_qr_code_pairs_without_the_relay_and_connects() {
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
                get(|| async { Json(json!({"data": [{"id": "qr-test-model"}]})) }),
            );
        let backend = tokio::spawn(axum::serve(backend_listener, backend).into_future());
        let scan = |(width, height, pixels): (u32, u32, Vec<u8>)| {
            let mut frame = width.to_le_bytes().to_vec();
            frame.extend(height.to_le_bytes());
            frame.extend(pixels);
            panel
                .http
                .post(format!("{}/api/scan", panel.origin))
                .bearer_auth(&panel.token)
                .body(frame)
                .send()
        };
        // Larger than the 64 KiB the other routes accept.
        let nothing = scan((320, 240, vec![200; 320 * 240])).await.unwrap();
        assert_eq!(nothing.json::<Value>().await.unwrap()["found"], false);
        let foreign = scan(crate::qr::picture("https://example.com/menu", 4))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::BAD_REQUEST);
        let mismatched = panel
            .http
            .post(format!("{}/api/scan", panel.origin))
            .bearer_auth(&panel.token)
            .body(vec![9u8; 20])
            .send()
            .await
            .unwrap();
        assert_eq!(mismatched.status(), StatusCode::BAD_REQUEST);
        // The relay only carries the tunnel afterwards: nothing waits on the host here. The
        // handler is called directly, with the panel that trusts the test relay's CA.
        let text = relay.offer_qr();
        let frame = |(width, height, pixels): (u32, u32, Vec<u8>)| {
            let mut frame = width.to_le_bytes().to_vec();
            frame.extend(height.to_le_bytes());
            frame.extend(pixels);
            axum::body::Bytes::from(frame)
        };
        let paired_now =
            super::scan(State(panel.clone()), frame(crate::qr::picture(&text, 4))).await;
        assert_eq!(paired_now.status(), StatusCode::OK);
        assert_eq!(paired(&directory).len(), 1);
        let mut host = Config::load(&relay.root.join("pairing/host/host.toml")).unwrap();
        host.upstream = format!("http://{backend_address}");
        let remote_stop = CancellationToken::new();
        let host_task = tokio::spawn(crate::host::run(
            host,
            remote_stop.clone(),
            relay.root.join("host-pairing-state"),
        ));
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        panel.control.lock().await.connections[0].config.listen = api.local_addr().unwrap();
        drop(api);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let state = status(State(panel.clone())).await.0;
                if state["phase"] == "connected" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the tunnel connects from the QR code's material alone");
        let response = test_backend(State(panel.clone())).await;
        let bytes = axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap();
        let result: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(result["models"][0], "qr-test-model");
        // Scanning again while connected is refused.
        let again = super::scan(State(panel.clone()), frame(crate::qr::picture(&text, 4))).await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
        stop_client(&panel).await;
        remote_stop.cancel();
        let _ = host_task.await.unwrap();
        backend.abort();
        cleanup(panel, ui_task, directory).await;
    }

    /// A model computer for the multi-host test: its own relay, host and backend that
    /// names itself in every answer.
    async fn computer(
        name: &'static str,
        models: &'static [&'static str],
    ) -> (
        crate::mailbox::couple::tests::Fixture,
        String,
        CancellationToken,
        JoinHandle<()>,
        JoinHandle<anyhow::Result<()>>,
    ) {
        let relay = crate::mailbox::couple::tests::fixture().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
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
                get(move || async move {
                    Json(json!({"data": models.iter().map(|id| json!({"id": id})).collect::<Vec<_>>()}))
                }),
            )
            .route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| async move {
                    Json(json!({"computer": name, "model": body["model"]}))
                }),
            );
        let backend = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
        let text = relay.offer_qr();
        let mut host = Config::load(&relay.root.join("pairing/host/host.toml")).unwrap();
        host.upstream = format!("http://{address}");
        let stop = CancellationToken::new();
        let task = tokio::spawn(crate::host::run(
            host,
            stop.clone(),
            relay.root.join("host-pairing-state"),
        ));
        (relay, text, stop, backend, task)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn several_computers_share_one_local_api_and_never_take_each_others_requests() {
        let (mut panel, ui_task, directory) = fixture().await;
        let (relay_a, qr_a, stop_a, backend_a, host_a) =
            computer("A", &["model-a", "shared"]).await;
        let (relay_b, qr_b, stop_b, backend_b, host_b) =
            computer("B", &["model-b", "shared"]).await;
        // Both test relays have their own CA; the panel trusts both.
        let bundle = directory.join("relays.pem");
        let pems = std::fs::read_to_string(&relay_a.ca).unwrap()
            + &std::fs::read_to_string(&relay_b.ca).unwrap();
        std::fs::write(&bundle, pems).unwrap();
        panel.relay_ca = Some(bundle);
        let frame = |text: &str| {
            let (width, height, pixels) = crate::qr::picture(text, 4);
            let mut frame = width.to_le_bytes().to_vec();
            frame.extend(height.to_le_bytes());
            frame.extend(pixels);
            axum::body::Bytes::from(frame)
        };
        let paired_a = super::scan(State(panel.clone()), frame(&qr_a)).await;
        assert_eq!(paired_a.status(), StatusCode::OK);
        let api = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = api.local_addr().unwrap();
        panel.control.lock().await.connections[0].config.listen = listen;
        drop(api);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        // The key an AI app got for the first computer stays the key of the shared API.
        let first_key = panel.control.lock().await.connections[0]
            .config
            .api_key
            .clone();
        assert_eq!(panel.key().as_deref(), Some(first_key.as_str()));
        // A second computer joins while the first stays connected.
        let paired_b = super::scan(State(panel.clone()), frame(&qr_b)).await;
        assert_eq!(paired_b.status(), StatusCode::OK);
        let again = super::scan(State(panel.clone()), frame(&qr_b)).await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
        assert_eq!(connect(State(panel.clone())).await.status(), StatusCode::OK);
        assert_eq!(paired(&directory).len(), 2);
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let state = status(State(panel.clone())).await.0;
                let computers = state["computers"].as_array().cloned().unwrap_or_default();
                if computers.len() == 2 && computers.iter().all(|c| c["phase"] == "connected") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("both computers connect");
        let http = &panel.http;
        let models: Value = http
            .get(format!("http://{listen}/v1/models"))
            .bearer_auth(&first_key)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut ids: Vec<_> = models["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        assert_eq!(ids, ["model-a", "model-b", "shared", "shared@2"]);
        let ask = |model: &'static str| {
            http.post(format!("http://{listen}/v1/chat/completions"))
                .bearer_auth(&first_key)
                .json(&json!({"model": model, "messages": [{"role": "user", "content": "hi"}]}))
                .send()
        };
        let answer = |response: reqwest::Response| async move {
            let value: Value = response.json().await.unwrap();
            (
                value["computer"].as_str().unwrap_or_default().to_owned(),
                value["model"].clone(),
            )
        };
        assert_eq!(answer(ask("model-b").await.unwrap()).await.0, "B");
        assert_eq!(answer(ask("model-a").await.unwrap()).await.0, "A");
        assert_eq!(
            answer(ask("shared").await.unwrap()).await,
            ("A".into(), json!("shared"))
        );
        // The second computer's copy of a shared model, under its own name there.
        assert_eq!(
            answer(ask("shared@2").await.unwrap()).await,
            ("B".into(), json!("shared"))
        );
        // Unknown models go to the first computer, as with one connection.
        assert_eq!(answer(ask("unlisted").await.unwrap()).await.0, "A");
        let wrong_key = http
            .get(format!("http://{listen}/v1/models"))
            .bearer_auth("not-the-key")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong_key.status(), StatusCode::UNAUTHORIZED);
        // B goes away: its model is not answered by A.
        stop_b.cancel();
        let _ = host_b.await;
        backend_b.abort();
        // It waits for B to come back, then fails; it is never answered by A.
        match ask("model-b").await {
            Err(error) => assert!(error.is_timeout()),
            Ok(response) => assert!(response.status().is_server_error(), "{}", response.status()),
        }
        assert_eq!(answer(ask("model-a").await.unwrap()).await.0, "A");
        // Removing B deletes its pairing; A keeps running.
        let b = panel.control.lock().await.connections[1].id.clone();
        let removed = remove(
            State(panel.clone()),
            Ok(Json(RemoveInput { id: b.clone() })),
        )
        .await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert!(!directory.join(&b).exists());
        assert_eq!(paired(&directory).len(), 1);
        assert_eq!(answer(ask("model-a").await.unwrap()).await.0, "A");
        stop_client(&panel).await;
        stop_a.cancel();
        let _ = host_a.await;
        backend_a.abort();
        drop((relay_a, relay_b));
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
        panel.control.lock().await.connections = vec![Connection::new(IMPORTED.into(), config)];
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

    /// A stand-in for the local gateway: it checks the key and echoes the request it got.
    async fn chat_fixture() -> (Panel, JoinHandle<()>, PathBuf, JoinHandle<()>) {
        let (panel, task, directory) = fixture().await;
        let gateway = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = config();
        cfg.listen = gateway.local_addr().unwrap();
        let key = cfg.api_key.clone();
        let echo = Router::new().route(
            "/v1/chat/completions",
            post(
                move |headers: axum::http::HeaderMap, body: axum::body::Bytes| async move {
                    let authorized = headers["authorization"] == format!("Bearer {key}").as_str()
                        && !headers.contains_key("origin");
                    if !authorized {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    let request: Value = serde_json::from_slice(&body).unwrap();
                    let reply = format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({"echo": request, "size": body.len()})
                    );
                    ([("content-type", "text/event-stream")], reply).into_response()
                },
            ),
        );
        let backend = tokio::spawn(async move { axum::serve(gateway, echo).await.unwrap() });
        let stop = panel.stop.child_token();
        let waiting = stop.clone();
        *panel.key.write().unwrap() = Some(cfg.api_key.clone());
        let mut control = panel.control.lock().await;
        control.connections = vec![Connection::new(IMPORTED.into(), cfg)];
        control.front = Some(Active {
            stop,
            task: tokio::spawn(async move { waiting.cancelled().await }),
        });
        drop(control);
        (panel, task, directory, backend)
    }

    #[tokio::test]
    async fn a_chat_streams_through_the_gateway_with_its_key_and_long_histories_fit() {
        let (panel, task, directory, backend) = chat_fixture().await;
        let url = format!("{}/api/chat", panel.origin);
        for path in ["chat", "chats", "chats/00ff"] {
            let request = panel.http.get(format!("{}/api/{path}", panel.origin));
            assert_eq!(
                request.send().await.unwrap().status(),
                StatusCode::UNAUTHORIZED
            );
        }
        // Longer than the 64 KiB every other panel route accepts.
        let history = "x".repeat(200 * 1024);
        let response = panel
            .http
            .post(&url)
            .bearer_auth(&panel.token)
            .json(&json!({"messages": [{"role": "user", "content": history}],
                "reasoning_effort": "low", "chat_template_kwargs": {"reasoning_effort": "low"}}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let text = response.text().await.unwrap();
        let echo: Value =
            serde_json::from_str(text.lines().next().unwrap().strip_prefix("data: ").unwrap())
                .unwrap();
        assert_eq!(echo["echo"]["stream"], true);
        assert_eq!(echo["echo"]["stream_options"]["include_usage"], true);
        assert_eq!(echo["echo"]["reasoning_effort"], "low");
        assert_eq!(
            echo["echo"]["chat_template_kwargs"]["reasoning_effort"],
            "low"
        );
        assert!(echo["size"].as_u64().unwrap() > 200 * 1024);
        assert!(text.ends_with("data: [DONE]\n\n"));
        let too_long_elsewhere = panel
            .http
            .post(format!("{}/api/config", panel.origin))
            .bearer_auth(&panel.token)
            .json(&json!({"toml": history}))
            .send()
            .await
            .unwrap();
        assert_eq!(too_long_elsewhere.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let invalid = panel
            .http
            .post(&url)
            .bearer_auth(&panel.token)
            .json(&json!({"prompt": "no messages"}))
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        stop_client(&panel).await;
        let stopped = panel
            .http
            .post(&url)
            .bearer_auth(&panel.token)
            .json(&json!({"messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(stopped.status(), StatusCode::CONFLICT);
        backend.abort();
        cleanup(panel, task, directory).await;
    }

    #[tokio::test]
    async fn the_chosen_language_outlives_the_page_and_only_known_ones_are_kept() {
        let (panel, task, directory) = fixture().await;
        let send = |request: reqwest::RequestBuilder| request.bearer_auth(&panel.token).send();
        let status = || async {
            send(panel.http.get(format!("{}/api/status", panel.origin)))
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap()
        };
        let choose = |language: &str| {
            send(
                panel
                    .http
                    .post(format!("{}/api/language", panel.origin))
                    .json(&json!({ "language": language })),
            )
        };
        // Nothing chosen: the page follows the browser.
        assert!(status().await.get("language").is_none());
        for language in ["en", "de"] {
            assert_eq!(choose(language).await.unwrap().status(), StatusCode::OK);
            assert_eq!(status().await["language"], language);
        }
        assert_eq!(
            std::fs::read_to_string(directory.join(LANGUAGE)).unwrap(),
            "de\n"
        );
        for unknown in ["fr", "", "../de"] {
            let response = choose(unknown).await.unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{unknown}");
        }
        assert_eq!(status().await["language"], "de");
        cleanup(panel, task, directory).await;
    }

    #[tokio::test]
    async fn a_saved_chat_is_a_private_file_until_it_is_deleted() {
        let (panel, task, directory) = fixture().await;
        let chat = |id: &str| format!("{}/api/chats/{id}", panel.origin);
        let send = |request: reqwest::RequestBuilder| request.bearer_auth(&panel.token).send();
        let saved = json!({"title": "Primes", "updated": 2, "messages": [{"role": "user", "content": "hi"}]});
        let response = send(panel.http.put(chat("0a1b")).json(&saved))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let file = directory.join("chats/0a1b.json");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode(&file) & 0o777, 0o600);
            assert_eq!(mode(&directory.join("chats")) & 0o777, 0o700);
        }
        // Saving again replaces the copy whole.
        let newer = json!({"title": "Primes", "updated": 3, "messages": []});
        send(panel.http.put(chat("0a1b")).json(&newer))
            .await
            .unwrap();
        let older = json!({"title": "Older", "updated": 1, "messages": []});
        send(panel.http.put(chat("ff")).json(&older)).await.unwrap();
        let list: Value = send(panel.http.get(format!("{}/api/chats", panel.origin)))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(list["chats"][0]["id"], "0a1b");
        assert_eq!(list["chats"][0]["messages"], 0);
        assert_eq!(list["chats"][1]["title"], "Older");
        let loaded: Value = send(panel.http.get(chat("0a1b")))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(loaded, newer);
        for id in ["..%2Fclient", "not-hex", &"a".repeat(33)] {
            let response = send(panel.http.put(chat(id)).json(&saved)).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{id}");
        }
        let invalid = send(
            panel
                .http
                .put(chat("0a1b"))
                .json(&json!({"messages": "no"})),
        )
        .await
        .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        // A renamed chat keeps its new title; a title the page would never send is refused.
        let renamed =
            json!({"title": "Ü".repeat(chat::MAX_TITLE_CHARS), "updated": 4, "messages": []});
        let response = send(panel.http.put(chat("0a1b")).json(&renamed))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let too_long =
            json!({"title": "a".repeat(chat::MAX_TITLE_CHARS + 1), "updated": 5, "messages": []});
        let response = send(panel.http.put(chat("0a1b")).json(&too_long))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let loaded: Value = send(panel.http.get(chat("0a1b")))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(loaded, renamed);
        let deleted = send(panel.http.delete(chat("0a1b"))).await.unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        assert!(!file.exists());
        let gone = send(panel.http.get(chat("0a1b"))).await.unwrap();
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        cleanup(panel, task, directory).await;
    }
}
