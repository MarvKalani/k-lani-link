//! The panel's chat windows: replies stream through the local gateway, and a chat is
//! written to disk only when the user saves it, in the panel's private directory.
use super::{Panel, failure};
use crate::{
    config::{private_file, random_secret},
    pairing::private_directory,
};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

/// A long chat is sent whole with every message; the gateway accepts the same size.
pub const MAX_CHAT_BYTES: usize = k_lani_link_protocol::MAX_BODY_BYTES;
const MAX_SAVED: usize = 200;
const DIRECTORY: &str = "chats";

pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/api/chat", post(chat))
        .route("/api/chats", get(list))
        .route("/api/chats/{id}", get(load).put(save).delete(remove))
        .layer(DefaultBodyLimit::max(MAX_CHAT_BYTES))
}

/// Forwards one chat request to the local gateway and streams its answer back. The page
/// never sees the API key, and the panel lock is not held while the answer streams.
async fn chat(State(panel): State<Panel>, body: Bytes) -> Response {
    let target = {
        let control = panel.control.lock().await;
        control
            .config
            .as_ref()
            .filter(|_| control.active.is_some())
            .map(|config| (config.listen, config.api_key.clone()))
    };
    let Some((listen, key)) = target else {
        return failure(StatusCode::CONFLICT, "Start the connection first.");
    };
    let mut request = serde_json::from_slice::<Value>(&body).unwrap_or_default();
    let Some(fields) = request
        .as_object_mut()
        .filter(|fields| fields.get("messages").is_some_and(Value::is_array))
    else {
        return failure(StatusCode::BAD_REQUEST, "This is not a valid chat request.");
    };
    fields.insert("stream".into(), json!(true));
    fields.insert("stream_options".into(), json!({"include_usage": true}));
    let response = panel
        .stream
        .post(format!("http://{listen}/v1/chat/completions"))
        .bearer_auth(key)
        .json(&request)
        .send()
        .await;
    let Ok(response) = response else {
        return failure(
            StatusCode::BAD_GATEWAY,
            "The connection did not answer. Try again.",
        );
    };
    let content_type = response
        .headers()
        .get("content-type")
        .cloned()
        .unwrap_or_else(|| "application/json".parse().unwrap());
    let status = response.status();
    let mut answer = Body::from_stream(response.bytes_stream()).into_response();
    *answer.status_mut() = status;
    answer.headers_mut().insert("content-type", content_type);
    answer
}

fn file(panel: &Panel, id: &str) -> Option<std::path::PathBuf> {
    let valid = (1..=32).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then(|| panel.directory.join(DIRECTORY).join(format!("{id}.json")))
}

/// The saved chats, newest first, without their messages.
async fn list(State(panel): State<Panel>) -> Response {
    let directory = panel.directory.join(DIRECTORY);
    let shown = directory.display().to_string();
    let chats = tokio::task::spawn_blocking(move || {
        let mut chats: Vec<Value> = std::fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let id = name.strip_suffix(".json")?.to_owned();
                let chat: Value =
                    serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
                Some(
                    json!({"id": id, "title": chat["title"], "updated": chat["updated"],
                    "messages": chat["messages"].as_array().map_or(0, Vec::len)}),
                )
            })
            .collect();
        chats.sort_by(|a, b| {
            b["updated"]
                .as_f64()
                .partial_cmp(&a["updated"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        chats
    })
    .await
    .unwrap_or_default();
    Json(json!({"directory": shown, "chats": chats})).into_response()
}

async fn load(State(panel): State<Panel>, Path(id): Path<String>) -> Response {
    let Some(path) = file(&panel, &id) else {
        return failure(StatusCode::NOT_FOUND, "This chat does not exist.");
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => ([("content-type", "application/json")], bytes).into_response(),
        Err(_) => failure(StatusCode::NOT_FOUND, "This chat does not exist."),
    }
}

/// Writes the chat to a private file next to the others, replacing an earlier copy whole.
async fn save(State(panel): State<Panel>, Path(id): Path<String>, body: Bytes) -> Response {
    let Some(path) = file(&panel, &id) else {
        return failure(StatusCode::NOT_FOUND, "This chat does not exist.");
    };
    let valid = serde_json::from_slice::<Value>(&body)
        .ok()
        .filter(|chat| chat["messages"].is_array() && chat["title"].is_string());
    let Some(chat) = valid else {
        return failure(StatusCode::BAD_REQUEST, "This is not a valid chat.");
    };
    let directory = panel.directory.join(DIRECTORY);
    let written = tokio::task::spawn_blocking(move || {
        private_directory(&directory)?;
        if !path.exists() && std::fs::read_dir(&directory)?.count() >= MAX_SAVED {
            return Ok(false);
        }
        // Each save writes its own file, so two saves of one chat never collide.
        let partial = path.with_extension(format!("{}.part", random_secret()?));
        private_file(&partial, &chat.to_string())?;
        std::fs::rename(&partial, &path)?;
        anyhow::Ok(true)
    })
    .await;
    match written {
        Ok(Ok(true)) => Json(json!({"saved": true})).into_response(),
        Ok(Ok(false)) => failure(
            StatusCode::CONFLICT,
            "Too many saved chats. Delete some first.",
        ),
        _ => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The chat could not be saved.",
        ),
    }
}

async fn remove(State(panel): State<Panel>, Path(id): Path<String>) -> Response {
    let Some(path) = file(&panel, &id) else {
        return failure(StatusCode::NOT_FOUND, "This chat does not exist.");
    };
    match tokio::fs::remove_file(path).await {
        Ok(()) => Json(json!({"deleted": true})).into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Json(json!({"deleted": true})).into_response()
        }
        Err(_) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The chat could not be deleted.",
        ),
    }
}
