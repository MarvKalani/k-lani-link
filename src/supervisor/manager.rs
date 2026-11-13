//! The model supervisor inside the network-less model container: loads one allowlisted model
//! at a time, Basalt or llama.cpp, and answers on a private control socket. Only catalog IDs
//! cross that socket; paths and launch flags come from the operator's catalog alone.
use super::catalog::{Catalog, Engine, Prefill, Profile, model_id};
use anyhow::Result;
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const MAX_COMMAND_BYTES: usize = 1024;
const MAX_REPLY_BYTES: usize = 64 * 1024;
pub const BASALT: [&str; 6] = [
    "/opt/basalt/.venv/bin/python",
    "-B",
    "-m",
    "serve.server",
    "--engine-bin",
    "/opt/basalt/bin/basalt",
];
pub const LLAMA: &str = "/opt/llama/llama-server";

/// How a model process comes to life: a real child process, or a stand-in in tests.
pub trait Launcher: Send + Sync {
    fn spawn(
        &self,
        program: &[String],
        env: Option<Vec<(String, String)>>,
    ) -> Result<Box<dyn Process>>;
}
/// A started model process.
pub trait Process: Send + Sync {
    /// Still running: no exit code yet.
    fn running(&mut self) -> bool;
    fn exit_code(&mut self) -> Option<i32>;
    /// Ends it and everything it started, politely first.
    fn stop(&mut self, grace: Duration);
}

pub struct Settings {
    pub runtime: PathBuf,
    pub basalt: Vec<String>,
    /// Basalt 0.3.3 and later accept a token reserve in place of a Boolean switch.
    pub basalt_answer_room_value: bool,
    pub llama: Vec<String>,
    pub metrics: bool,
    pub timeout: Duration,
    pub require_no_swap: bool,
    /// Asked again before every load: swap may be switched on while a model runs.
    pub swap_disabled: fn() -> bool,
}
impl Settings {
    pub fn production(runtime: PathBuf, metrics: bool, require_no_swap: bool) -> Self {
        Self {
            runtime,
            basalt: BASALT.iter().map(|s| (*s).to_owned()).collect(),
            basalt_answer_room_value: std::fs::read_to_string(super::BASALT_OPTIONS)
                .is_ok_and(|options| options.contains("type=answer_room")),
            llama: vec![LLAMA.to_owned()],
            metrics,
            timeout: Duration::from_secs(180),
            require_no_swap,
            swap_disabled: super::host_swap_disabled,
        }
    }
}

pub struct Manager {
    catalog: Catalog,
    settings: Settings,
    launcher: Box<dyn Launcher>,
    process: Option<Box<dyn Process>>,
    active: Option<String>,
    stopping: CancellationToken,
}

fn socket_client(socket: &Path) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .unix_socket(socket)
        .timeout(Duration::from_secs(1))
        .build()
        .ok()
}
async fn get_json(client: &reqwest::Client, route: &str) -> Option<Value> {
    let response = client
        .get(format!("http://localhost{route}"))
        .send()
        .await
        .ok()?;
    if response.status() != StatusCode::OK {
        return None;
    }
    let bytes = crate::mailbox::response_bytes(response, MAX_REPLY_BYTES)
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}
/// Basalt: its own privacy profile must be on, and the model loaded.
async fn basalt_ready(socket: &Path) -> Option<Value> {
    let health = get_json(&socket_client(socket)?, "/health").await?;
    let privacy = &health["link_privacy"];
    let private = privacy["version"] == 1
        && [
            "application_logs",
            "request_history",
            "diagnostics",
            "remote_images",
        ]
        .iter()
        .all(|key| privacy[*key] == false);
    (private && health["loaded"] == true).then_some(health)
}
/// llama.cpp has no privacy profile; the supervisor vouches for how it started it.
async fn llama_ready(socket: &Path, name: &str) -> Option<Value> {
    let client = socket_client(socket)?;
    let health = get_json(&client, "/health").await?;
    if health.is_null() || health.as_object().is_some_and(|o| o.is_empty()) {
        return None;
    }
    let props = get_json(&client, "/props").await.unwrap_or_default();
    let ids: Vec<Value> = get_json(&client, "/v1/models")
        .await
        .and_then(|list| list["data"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|row| row["id"].clone())
        .collect();
    let model = if ids == [json!(name)] {
        json!(name)
    } else {
        json!(ids)
    };
    Some(
        json!({"loaded": true, "model": model, "parallel": props["total_slots"],
        "max_context": props["default_generation_settings"]["n_ctx"]}),
    )
}

/// Removes a leftover socket of this user; anything else in its place is refused.
pub fn remove_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let info = match std::fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        info.file_type().is_socket() && info.uid() == unsafe { libc::getuid() },
        "unsafe socket path"
    );
    std::fs::remove_file(path)?;
    Ok(())
}

/// The launch words for one profile, as the engine reads them.
pub fn launch_args(settings: &Settings, profile: &Profile) -> Vec<String> {
    let socket = settings.runtime.join("basalt.sock").display().to_string();
    let s = |v: &str| v.to_owned();
    let mut args: Vec<String>;
    match profile.engine {
        Engine::Llama => {
            // A host ending in .sock makes llama.cpp listen there instead of on TCP.
            args = settings.llama.clone();
            args.extend([
                s("--model"),
                profile.path.clone(),
                s("--alias"),
                profile.id.clone(),
                s("--host"),
                socket,
                s("--ctx-size"),
                profile.context_tokens.to_string(),
                s("--parallel"),
                profile.parallel.to_string(),
                s("--kv-unified"),
                s("--n-gpu-layers"),
                s("all"),
                s("--flash-attn"),
                s("on"),
                s("--cache-type-k"),
                profile.kv.clone(),
                s("--cache-type-v"),
                profile.kv.clone(),
                s("--jinja"),
                s("--reasoning-budget"),
                profile.reasoning_budget_tokens.to_string(),
                s("--no-webui"),
                s("--no-slots"),
                s("--log-disable"),
                s("--offline"),
            ]);
        }
        Engine::Basalt => {
            args = settings.basalt.clone();
            args.extend([
                s("--model"),
                profile.path.clone(),
                s("--alias"),
                profile.id.clone(),
                s("--privacy"),
                s("--vision"),
                s("off"),
                s("--host"),
                s("127.0.0.1"),
                s("--unix-socket"),
                socket,
                s("--parallel"),
                profile.parallel.to_string(),
                s("--max-context"),
                profile.context_tokens.to_string(),
                s("--kv"),
                s("int8"),
                s(if profile.resident {
                    "--resident-experts"
                } else {
                    "--mmap-experts"
                }),
                s("--prefill"),
                match &profile.prefill {
                    Prefill::Tokens(n) => n.to_string(),
                    Prefill::Auto(auto) => auto.clone(),
                },
                s("--reasoning-budget-tokens"),
                profile.reasoning_budget_tokens.to_string(),
            ]);
            if profile.slot_full_context {
                args.push(s("--slot-full-context"));
            }
            if let Some(file) = &profile.expert_profile {
                args.extend([s("--expert-profile"), file.clone()]);
            }
            if let Some(share) = &profile.prefill_cpu_share {
                args.extend([s("--prefill-cpu-share"), share.to_string()]);
            }
            if let Some(workers) = profile.pool_workers {
                args.extend([s("--pool-workers"), workers.to_string()]);
            }
            if let Some(enabled) = profile.answer_room {
                if settings.basalt_answer_room_value {
                    args.extend([s("--answer-room"), s(if enabled { "auto" } else { "0" })]);
                } else {
                    args.push(s(if enabled {
                        "--answer-room"
                    } else {
                        "--no-answer-room"
                    }));
                }
            }
            if profile.fit_max_tokens {
                args.push(s("--fit-max-tokens"));
            }
            for (key, value) in &profile.sampling {
                args.extend([format!("--{}", key.replace('_', "-")), value.to_string()]);
            }
            if settings.metrics {
                args.extend([
                    s("--privacy-metrics-file"),
                    settings
                        .runtime
                        .join("performance.json")
                        .display()
                        .to_string(),
                ]);
            }
        }
    }
    args
}

impl Manager {
    pub fn new(catalog: Catalog, settings: Settings, launcher: Box<dyn Launcher>) -> Self {
        Self {
            catalog,
            settings,
            launcher,
            process: None,
            active: None,
            stopping: CancellationToken::new(),
        }
    }
    pub fn stopping(&self) -> CancellationToken {
        self.stopping.clone()
    }
    fn socket(&self) -> PathBuf {
        self.settings.runtime.join("basalt.sock")
    }
    async fn ready(&self, name: &str) -> Option<Value> {
        match self.catalog.get(name)?.engine {
            Engine::Llama => llama_ready(&self.socket(), name).await,
            Engine::Basalt => basalt_ready(&self.socket()).await,
        }
    }

    pub async fn snapshot(&mut self) -> Value {
        let running = self.active.is_some() && self.process.as_mut().is_some_and(|p| p.running());
        let mut active = None;
        if running
            && let Some(name) = self.active.clone()
            && self.ready(&name).await.is_some()
        {
            active = Some(name);
        }
        // "basalt": the engine's own privacy profile passed; "supervised": this supervisor
        // started an engine without logs, web UI, slot monitor or network.
        let models: Vec<Value> = self
            .catalog
            .models
            .iter()
            .map(|p| {
                let loaded = active.as_deref() == Some(p.id.as_str());
                json!({"id": p.id, "engine": p.engine.name(), "context_tokens": p.context_tokens,
                    "parallel": p.parallel, "context_per_slot": p.context_per_slot(), "loaded": loaded,
                    "privacy": loaded.then_some(match p.engine { Engine::Basalt => "basalt", Engine::Llama => "supervised" })})
            })
            .collect();
        json!({"active_model": active, "models": models})
    }

    pub fn stop_engine(&mut self) {
        self.active = None;
        if let Some(mut process) = self.process.take() {
            process.stop(Duration::from_secs(10));
        }
        let _ = remove_socket(&self.socket());
        for name in ["performance.json", "performance.json.tmp"] {
            let path = self.settings.runtime.join(name);
            if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    pub async fn load(&mut self, name: &str) -> (StatusCode, Value) {
        let Some(profile) = self.catalog.get(name).cloned() else {
            return (StatusCode::NOT_FOUND, json!({"error": "unknown model ID"}));
        };
        if self.stopping.is_cancelled() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": "backend is stopping"}),
            );
        }
        if self.settings.require_no_swap && !(self.settings.swap_disabled)() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error": "host swap must be disabled before loading a model"}),
            );
        }
        if self.active.as_deref() == Some(name) && self.ready(name).await.is_some() {
            return (StatusCode::OK, self.snapshot().await);
        }
        self.stop_engine();
        let args = launch_args(&self.settings, &profile);
        let env = (profile.engine == Engine::Llama).then(|| {
            // Its own libraries next to it, its CUDA 12 runtime in lib/; nothing else uses them.
            let folder = Path::new(&self.settings.llama[0])
                .parent()
                .unwrap_or(Path::new("/"));
            let mut env: Vec<(String, String)> = std::env::vars()
                .filter(|(key, _)| key != "LD_LIBRARY_PATH")
                .collect();
            env.push((
                "LD_LIBRARY_PATH".into(),
                format!("{}:{}", folder.join("lib").display(), folder.display()),
            ));
            env
        });
        self.process = match self.launcher.spawn(&args, env) {
            Ok(process) => Some(process),
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"error": "model failed to become ready; backend remains unloaded", "exit_code": null}),
                );
            }
        };
        let deadline = tokio::time::Instant::now() + self.settings.timeout;
        while !self.stopping.is_cancelled()
            && tokio::time::Instant::now() < deadline
            && self.process.as_mut().is_some_and(|p| p.running())
        {
            if let Some(ready) = self.ready(name).await {
                let context = profile.context_per_slot();
                if ready["model"] != json!(name)
                    || ready["parallel"] != json!(profile.parallel)
                    || ready["max_context"] != json!(context)
                {
                    self.stop_engine();
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        json!({"error": "backend could not provide the configured model, slots or context",
                            "actual_parallel": ready["parallel"], "actual_context": ready["max_context"]}),
                    );
                }
                self.active = Some(name.to_owned());
                return (StatusCode::OK, self.snapshot().await);
            }
            tokio::select! {
                _ = self.stopping.cancelled() => {}
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        let exit_code = self.process.as_mut().and_then(|p| p.exit_code());
        self.stop_engine();
        (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "model failed to become ready; backend remains unloaded", "exit_code": exit_code}),
        )
    }

    /// One control command: `{"action":"load","model":ID}` or `{"action":"unload"}` exactly.
    pub async fn command(&mut self, body: &Value) -> (StatusCode, Value) {
        let invalid = (
            StatusCode::BAD_REQUEST,
            json!({"error": "invalid model control command"}),
        );
        let Some(fields) = body.as_object() else {
            return invalid;
        };
        match (
            fields.len(),
            fields.get("action").and_then(Value::as_str),
            fields.get("model"),
        ) {
            (2, Some("load"), Some(Value::String(model))) if model_id(model) => {
                self.load(model).await
            }
            (1, Some("unload"), None) => {
                self.stop_engine();
                (StatusCode::OK, self.snapshot().await)
            }
            _ => invalid,
        }
    }
}

type Shared = Arc<Mutex<Manager>>;

fn reply(status: StatusCode, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_string()))
        .unwrap()
}
fn not_found() -> Response {
    reply(
        StatusCode::NOT_FOUND,
        &json!({"error": "route not available"}),
    )
}

async fn models(State(manager): State<Shared>) -> Response {
    let mut manager = manager.lock().await;
    reply(StatusCode::OK, &manager.snapshot().await)
}
async fn model(State(manager): State<Shared>, request: Request) -> Response {
    let invalid = || {
        reply(
            StatusCode::BAD_REQUEST,
            &json!({"error": "invalid model control command"}),
        )
    };
    let headers: &HeaderMap = request.headers();
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if headers.contains_key(header::TRANSFER_ENCODING)
        || !length.is_some_and(|n| (1..=MAX_COMMAND_BYTES).contains(&n))
    {
        return invalid();
    }
    let Ok(bytes) = to_bytes(request.into_body(), MAX_COMMAND_BYTES).await else {
        return invalid();
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return invalid();
    };
    let mut manager = manager.lock().await;
    let (status, result) = manager.command(&body).await;
    reply(status, &result)
}

pub fn router(manager: Shared) -> Router {
    Router::new()
        .route("/v1/link/models", get(models))
        .route("/v1/link/model", post(model))
        .fallback(|| async { not_found() })
        .method_not_allowed_fallback(|| async { not_found() })
        .with_state(manager)
}

/// Serves the control socket until `stop`, after loading the catalog's default model.
pub async fn serve(manager: Manager, control: &Path, stop: CancellationToken) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    remove_socket(control)?;
    let default = manager.catalog.default.clone();
    let stopping = manager.stopping();
    let shared: Shared = Arc::new(Mutex::new(manager));
    let listener = tokio::net::UnixListener::bind(control)?;
    std::fs::set_permissions(control, std::fs::Permissions::from_mode(0o600))?;
    if let Some(name) = default {
        let shared = shared.clone();
        // The socket already accepts; requests wait for this first load like any other.
        tokio::spawn(async move {
            let _ = shared.lock().await.load(&name).await;
        });
    }
    let ended = stop.clone();
    let watcher = stopping.clone();
    tokio::spawn(async move {
        ended.cancelled().await;
        watcher.cancel();
    });
    let served = axum::serve(listener, router(shared.clone()))
        .with_graceful_shutdown(async move { stopping.cancelled().await })
        .await;
    shared.lock().await.stop_engine();
    let _ = remove_socket(control);
    served.map_err(Into::into)
}

/// The real launcher: its own process group, no input or output, so a stop ends all of it.
pub struct Processes;
struct Child(std::process::Child);
impl Launcher for Processes {
    fn spawn(
        &self,
        program: &[String],
        env: Option<Vec<(String, String)>>,
    ) -> Result<Box<dyn Process>> {
        use std::os::unix::process::CommandExt;
        let mut command = std::process::Command::new(&program[0]);
        command
            .args(&program[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        if let Some(env) = env {
            command.env_clear().envs(env);
        }
        Ok(Box::new(Child(command.spawn()?)))
    }
}
impl Process for Child {
    fn running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
    fn exit_code(&mut self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        // A signal shows as its negative number, as Python's subprocess reported it.
        let status = self.0.try_wait().ok().flatten()?;
        status.code().or(status.signal().map(|signal| -signal))
    }
    fn stop(&mut self, grace: Duration) {
        let group = -(self.0.id() as i32);
        if self.running() {
            unsafe { libc::kill(group, libc::SIGTERM) };
            let deadline = std::time::Instant::now() + grace;
            while self.running() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if self.running() {
                unsafe { libc::kill(group, libc::SIGKILL) };
            }
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests;
