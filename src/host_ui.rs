//! The host setup page: `k-lani-link host-ui --config host.toml` opens it on this computer.
//! It shows the host's backends and models and edits the settings file (model cards, project,
//! rule sets, roles). It never writes the host configuration and never shows its keys.
use crate::{
    config::{Config, random_secret, token_matches},
    host_settings::Settings,
    routing::Routes,
};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const HTML: &str = include_str!("host_ui/index.html");
const JS: &str = include_str!("host_ui/host.js");
const CSS: &str = include_str!("ui/style.css");
const HOST_CSS: &str = include_str!("host_ui/host.css");

#[derive(Clone)]
struct Page {
    token: String,
    origin: String,
    config_file: PathBuf,
    config: Arc<Config>,
    /// `settings_file` from the configuration, or the suggested place next to it.
    settings_file: PathBuf,
    stop: CancellationToken,
}

/// Where the settings file is, and whether the host configuration names it.
pub fn settings_path(config: &Config, config_file: &Path) -> (PathBuf, bool) {
    match &config.settings_file {
        Some(path) => (path.clone(), true),
        None => {
            let folder = std::path::absolute(config_file)
                .ok()
                .and_then(|file| file.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            (folder.join("host-settings.json"), false)
        }
    }
}

fn failure(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": message.into()}))).into_response()
}

/// Only this page, from this address, with its token.
async fn protect(State(page): State<Page>, request: Request, next: Next) -> Response {
    let host = page.origin.strip_prefix("http://").unwrap_or_default();
    if request.headers().get("host").and_then(|v| v.to_str().ok()) != Some(host) {
        return failure(StatusCode::FORBIDDEN, "Unrecognized local address.");
    }
    if request
        .headers()
        .get("origin")
        .is_some_and(|v| v != page.origin.as_str())
    {
        return failure(StatusCode::FORBIDDEN, "Requests must come from this page.");
    }
    if request.uri().path().starts_with("/api/")
        && !token_matches(
            request
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            &page.token,
        )
    {
        return failure(
            StatusCode::UNAUTHORIZED,
            "Open the page from the application.",
        );
    }
    let mut response = next.run(request).await;
    for (name, value) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-content-type-options", "nosniff"),
        (
            "content-security-policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, HeaderValue::from_static(value));
    }
    response
}

/// The configuration as far as the page needs it: addresses and switches, no keys.
async fn overview(State(page): State<Page>) -> Json<Value> {
    let config = &page.config;
    let (_, linked) = settings_path(config, &page.config_file);
    let line = format!(
        "settings_file = {}",
        toml::Value::String(page.settings_file.display().to_string())
    );
    let backends: Vec<_> = config
        .backends
        .iter()
        .map(|backend| {
            json!({"upstream": backend.upstream, "video": backend.video, "speech": backend.speech,
                "transcriptions": backend.transcriptions, "image_edits": backend.image_edits,
                "unverified": backend.allow_unverified_backend})
        })
        .collect();
    let models = match Routes::new(config) {
        Ok(routes) => match routes.model_list().await {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)
                .map(|list| json!({"models": list["data"]}))
                .unwrap_or_else(|_| json!({"models_error": "The model list is not readable."})),
            Err(error) => json!({"models_error": error.to_string()}),
        },
        Err(error) => json!({"models_error": error.to_string()}),
    };
    Json(json!({
        "config": page.config_file.display().to_string(),
        "settings": page.settings_file.display().to_string(),
        "linked": linked,
        "line": line,
        "upstream": config.upstream,
        "unverified": config.allow_unverified_backend,
        "backends": backends,
        "models": models["models"],
        "models_error": models["models_error"],
    }))
}

async fn load(State(page): State<Page>) -> Response {
    match Settings::load(&page.settings_file) {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => failure(StatusCode::CONFLICT, format!("{error:#}")),
    }
}

async fn save(State(page): State<Page>, body: axum::body::Bytes) -> Response {
    let settings: Settings = match serde_json::from_slice(&body) {
        Ok(settings) => settings,
        Err(_) => return failure(StatusCode::BAD_REQUEST, "These settings are not readable."),
    };
    match settings.save(&page.settings_file) {
        Ok(()) => Json(json!({"saved": true})).into_response(),
        Err(error) => failure(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

async fn quit(State(page): State<Page>) -> Json<Value> {
    page.stop.cancel();
    Json(json!({"closing": true}))
}

fn router(page: Page) -> Router {
    let script = |body: &'static str| {
        get(move || async move { ([("content-type", "text/javascript; charset=utf-8")], body) })
    };
    let style = |body: &'static str| {
        get(move || async move { ([("content-type", "text/css; charset=utf-8")], body) })
    };
    Router::new()
        .route("/", get(|| async { Html(HTML) }))
        .route("/host.js", script(JS))
        .route("/style.css", style(CSS))
        .route("/host.css", style(HOST_CSS))
        .route("/api/overview", get(overview))
        .route("/api/settings", get(load).post(save))
        .route("/api/quit", post(quit))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn_with_state(page.clone(), protect))
        .with_state(page)
}

/// Serves the page until it is closed or `stop` fires.
pub async fn run(
    config_file: &Path,
    stop: CancellationToken,
    launch_browser: bool,
) -> anyhow::Result<()> {
    let config = Config::load(config_file)?;
    let (settings_file, _) = settings_path(&config, config_file);
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let origin = format!("http://{}", listener.local_addr()?);
    let token = random_secret()?;
    let address = format!("{origin}/#{token}");
    let page = Page {
        token,
        origin,
        config_file: config_file.to_path_buf(),
        config: Arc::new(config),
        settings_file,
        stop: stop.clone(),
    };
    if !launch_browser || crate::ui::open_browser(&address).is_err() {
        // Explicitly requested by the operator: the address with its token, for this computer.
        println!("Open the host setup page: {address}");
    }
    axum::serve(listener, router(page))
        .with_graceful_shutdown(stop.cancelled_owned())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_page_answers_only_with_its_token_and_saves_valid_settings_only() {
        let folder = std::env::temp_dir().join(format!("kl-host-ui-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let mut config = crate::config::tests::config();
        config.upstream = "http://127.0.0.1:9".into();
        let settings_file = folder.join("host-settings.json");
        config.settings_file = Some(settings_file.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let page = Page {
            token: "t".repeat(43),
            origin: origin.clone(),
            config_file: folder.join("host.toml"),
            config: Arc::new(config.clone()),
            settings_file: settings_file.clone(),
            stop: CancellationToken::new(),
        };
        tokio::spawn(async move { axum::serve(listener, router(page)).await.unwrap() });
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let api = |path: &str| format!("{origin}/api/{path}");
        assert_eq!(
            http.get(api("overview")).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let overview: Value = http
            .get(api("overview"))
            .bearer_auth("t".repeat(43))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(overview["linked"], true);
        let shown = overview.to_string();
        for secret in [&config.api_key, &config.pair_key, &config.signaling_token] {
            assert!(!shown.contains(secret.as_str()), "a key reached the page");
        }
        let bad = json!({"roles": [{"name": "Coder", "rule_sets": ["missing"]}]});
        let refused = http
            .post(api("settings"))
            .bearer_auth("t".repeat(43))
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert!(!settings_file.exists());
        let good = json!({"project": "Demo", "rule_sets": [{"name": "Code", "rules": "KISS"}],
            "roles": [{"name": "Coder", "rule_sets": ["Code"], "instructions": "private"}]});
        http.post(api("settings"))
            .bearer_auth("t".repeat(43))
            .json(&good)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert_eq!(Settings::load(&settings_file).unwrap().project, "Demo");
        let foreign = http
            .get(api("settings"))
            .bearer_auth("t".repeat(43))
            .header("origin", "https://example.org")
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
