//! The one local API in front of every connected model computer: one base URL and one
//! key for the AI app, the models of all computers in one list. A request goes to the
//! computer that lists its model and to no other, even when that one is down; a request
//! without a known model goes to the first computer, as with a single connection.
use super::{Panel, failure};
use crate::{config::token_matches, gateway::Gateway};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use k_lani_link_protocol::{MAX_BODY_BYTES, accepted_query, allowed_route, route_api};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};
use tower::ServiceExt;

/// One connected model computer as the front reaches it.
#[derive(Clone)]
pub(super) struct Route {
    /// The connection's folder name, stable while it exists.
    pub id: String,
    /// Its own local key; the AI app never sees it.
    pub key: String,
    pub gateway: Gateway,
}
pub(super) type Routes = Arc<RwLock<Vec<Route>>>;

/// Which computer lists which model, rebuilt at most every few seconds.
#[derive(Default)]
pub(super) struct Catalog {
    built: Option<Instant>,
    /// Model id as the app sees it → (route id, model id as that computer knows it).
    owners: HashMap<String, (String, String)>,
    listed: Vec<Value>,
    /// Per route id, the ids it lists.
    by_route: HashMap<String, Vec<String>>,
}
pub(super) type Catalogs = Arc<Mutex<Catalog>>;
const CATALOG_SECONDS: u64 = 30;

pub(super) fn router(panel: Panel) -> Router {
    Router::new()
        .route("/health", get(health))
        .fallback(proxy)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(panel)
}

fn authorized(headers: &HeaderMap, key: &str) -> bool {
    let header = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|key| format!("Bearer {key}"));
    token_matches(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        key,
    ) | token_matches(header.as_deref(), key)
}

/// A request handed to one computer's gateway in-process, with that gateway's own key.
async fn call(
    route: &Route,
    method: Method,
    uri: &str,
    headers: &HeaderMap,
    body: Body,
) -> Response {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        let kept = !matches!(
            name.as_str(),
            "authorization" | "x-api-key" | "host" | "origin" | "content-length"
        );
        if kept {
            request = request.header(name, value);
        }
    }
    let request = request
        .header("authorization", format!("Bearer {}", route.key))
        .body(body);
    match request {
        Ok(request) => match route.gateway.router().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        },
        Err(_) => failure(StatusCode::BAD_REQUEST, "invalid request"),
    }
}

/// The connection's own health, as its gateway reports it.
pub(super) async fn health_of(route: &Route) -> Option<Value> {
    let response = call(
        route,
        Method::GET,
        "/health",
        &HeaderMap::new(),
        Body::empty(),
    )
    .await;
    if response.status() != StatusCode::OK {
        return None;
    }
    let bytes = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn models_of(route: &Route) -> Option<Vec<Value>> {
    let response = call(
        route,
        Method::GET,
        "/v1/models",
        &HeaderMap::new(),
        Body::empty(),
    )
    .await;
    if response.status() != StatusCode::OK {
        return None;
    }
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value["data"].as_array().cloned()
}

/// A model card's README from the first computer that serves it: `{"id", "readme"}`.
pub(super) async fn model_card(routes: &Routes, key: &str) -> Option<Value> {
    let routes = routes.read().unwrap().clone();
    let path = format!("{}{key}", k_lani_link_protocol::MODEL_CARD_PATH);
    for route in &routes {
        let response = call(route, Method::GET, &path, &HeaderMap::new(), Body::empty()).await;
        if response.status() != StatusCode::OK {
            continue;
        }
        // 32 KiB of text, escaped as JSON.
        let Ok(bytes) = axum::body::to_bytes(response.into_body(), 256 * 1024).await else {
            continue;
        };
        if let Ok(card) = serde_json::from_slice::<Value>(&bytes)
            && card["readme"].as_str().is_some_and(|text| {
                text.len() as u64 <= crate::host_settings::MODEL_CARD_README_BYTES
            })
        {
            return Some(card);
        }
    }
    None
}

/// Asks every computer for its models. A model two computers list stays with the first;
/// the second one's is offered as `<id>@<n>`, n being that computer's place in the list.
pub(super) async fn refresh(routes: &Routes, catalogs: &Catalogs) {
    let routes = routes.read().unwrap().clone();
    let lists = futures_util::future::join_all(routes.iter().map(models_of)).await;
    let mut catalog = Catalog {
        built: Some(Instant::now()),
        ..Catalog::default()
    };
    for (place, (route, list)) in routes.iter().zip(lists).enumerate() {
        let mut ids = Vec::new();
        for mut model in list.into_iter().flatten() {
            let Some(original) = model["id"].as_str().map(str::to_owned) else {
                continue;
            };
            let shown = if catalog.owners.contains_key(&original) {
                format!("{original}@{}", place + 1)
            } else {
                original.clone()
            };
            model["id"] = json!(shown);
            catalog
                .owners
                .insert(shown.clone(), (route.id.clone(), original));
            ids.push(shown);
            catalog.listed.push(model);
        }
        catalog.by_route.insert(route.id.clone(), ids);
    }
    *catalogs.lock().unwrap() = catalog;
}

/// The models each connection lists, for the panel; empty before the first refresh.
pub(super) fn listed_by(catalogs: &Catalogs, id: &str) -> Vec<String> {
    catalogs
        .lock()
        .unwrap()
        .by_route
        .get(id)
        .cloned()
        .unwrap_or_default()
}

async fn fresh(panel: &Panel, force: bool) {
    let stale = panel.catalog.lock().unwrap().built.is_none_or(|built| {
        built.elapsed() > Duration::from_secs(if force { 5 } else { CATALOG_SECONDS })
    });
    if stale {
        refresh(&panel.routes, &panel.catalog).await;
    }
}

async fn health(State(panel): State<Panel>, headers: HeaderMap) -> Response {
    if !panel.key().is_some_and(|key| authorized(&headers, &key)) {
        return failure(StatusCode::UNAUTHORIZED, "invalid k-lani-link API key");
    }
    let routes = panel.routes.read().unwrap().clone();
    let mut connected = false;
    for route in &routes {
        connected |= health_of(route)
            .await
            .is_some_and(|health| health["connected"] == true);
    }
    Json(json!({"connected": connected, "computers": routes.len()})).into_response()
}

async fn proxy(State(panel): State<Panel>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    if !panel
        .key()
        .is_some_and(|key| authorized(&parts.headers, &key))
    {
        return failure(StatusCode::UNAUTHORIZED, "invalid k-lani-link API key");
    }
    // Native API consumers do not send browser Origin. No ambient browser access.
    if parts.headers.contains_key("origin") {
        return failure(
            StatusCode::FORBIDDEN,
            "browser origins are not enabled in the native MVP",
        );
    }
    let path = parts.uri.path();
    let uri = parts
        .uri
        .path_and_query()
        .map_or(path, |p| p.as_str())
        .to_owned();
    // Reject control, unknown routes and unexpected queries before reading any body.
    let api = route_api(parts.method.as_str(), path);
    if !allowed_route(parts.method.as_str(), path)
        || parts
            .uri
            .query()
            .is_some_and(|query| api.and_then(accepted_query) != Some(query))
    {
        return failure(StatusCode::NOT_FOUND, "route not available");
    }
    let upload = match panel.front_uploads.reserve() {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };
    let routes = panel.routes.read().unwrap().clone();
    let Some(first) = routes.first().cloned() else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "no model computer is connected",
        );
    };
    if parts.method == Method::GET && path == "/v1/models" {
        fresh(&panel, false).await;
        let listed = panel.catalog.lock().unwrap().listed.clone();
        if listed.is_empty() {
            // Nothing answered: the first computer's own answer explains why.
            return call(&first, parts.method, &uri, &parts.headers, Body::empty()).await;
        }
        return Json(json!({"object": "list", "data": listed})).into_response();
    }
    let bytes = match panel.front_uploads.read(body).await {
        Ok(bytes) => bytes,
        Err(error) => return error.into_response(),
    };
    // The slot bounds bodies being received; once read, the gateway's own admission
    // bounds the request, so a request waiting for its answer does not hold it.
    drop(upload);
    let mut request: Option<Value> = serde_json::from_slice(&bytes).ok();
    let model = request
        .as_ref()
        .and_then(|value| value["model"].as_str())
        .map(str::to_owned);
    let mut owner = None;
    if let Some(model) = &model {
        fresh(&panel, false).await;
        owner = panel.catalog.lock().unwrap().owners.get(model).cloned();
        if owner.is_none() {
            // A model loaded since the last look is found before falling back.
            fresh(&panel, true).await;
            owner = panel.catalog.lock().unwrap().owners.get(model).cloned();
        }
    }
    let (route, body) = match owner {
        Some((id, original)) => {
            let Some(route) = routes.iter().find(|route| route.id == id).cloned() else {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "the model computer for this model is no longer connected",
                );
            };
            // `<id>@<n>` is the front's own name; the computer knows the model as `<id>`.
            let body = match (&mut request, model.as_deref() != Some(original.as_str())) {
                (Some(value), true) => {
                    value["model"] = json!(original);
                    Body::from(serde_json::to_vec(value).unwrap_or_default())
                }
                _ => Body::from(bytes),
            };
            (route, body)
        }
        None => (first, Body::from(bytes)),
    };
    let mut response = call(&route, parts.method, &uri, &parts.headers, body).await;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
}
