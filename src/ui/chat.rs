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
    extract::{DefaultBodyLimit, Path, Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use k_lani_link_protocol::{
    IMAGE_EDITS_PATH, MODEL_CARD_PATH, SPEECH_PATH, TRANSCRIPTIONS_PATH, model_card_route,
    multipart_content_type, multipart_route, video_route,
};
use serde_json::{Value, json};

/// A long chat is sent whole with every message; the gateway accepts the same size.
pub const MAX_CHAT_BYTES: usize = k_lani_link_protocol::MAX_BODY_BYTES;
const MAX_SAVED: usize = 200;
/// The page offers 80 characters; the panel accepts some slack, never a whole document.
pub const MAX_TITLE_CHARS: usize = 120;
const DIRECTORY: &str = "chats";

pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/api/chat", post(chat))
        .route("/api/media/{*path}", get(media).post(media))
        .route("/api/chats", get(list))
        .route("/api/chats/{id}", get(load).put(save).delete(remove))
        .route("/api/model-card/{key}", get(model_card))
        .layer(DefaultBodyLimit::max(MAX_CHAT_BYTES))
}

/// Forwards one chat request to the local gateway and streams its answer back. The page
/// never sees the API key, and the panel lock is not held while the answer streams.
async fn chat(State(panel): State<Panel>, request: Request) -> Response {
    let upload = match panel.chat_uploads.reserve() {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };
    let body = match panel.chat_uploads.read(request.into_body()).await {
        Ok(body) => body,
        Err(error) => return error.into_response(),
    };
    // The slot bounds bodies being received; once read, the gateway's own admission
    // bounds the request, so a request waiting for its answer does not hold it.
    drop(upload);
    // Through the front, like any AI app: the model picks the computer.
    let target = panel.gateway_address().await.zip(panel.key());
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
    gateway_response(
        panel
            .stream
            .post(format!("http://{listen}/v1/chat/completions"))
            .bearer_auth(key)
            .json(&request),
    )
    .await
}

/// A model card's README, from whichever connected computer serves it.
async fn model_card(State(panel): State<Panel>, Path(key): Path<String>) -> Response {
    if !model_card_route("GET", &format!("{MODEL_CARD_PATH}{key}")) {
        return failure(StatusCode::NOT_FOUND, "No description for this model.");
    }
    match super::front::model_card(&panel.routes, &key).await {
        Some(card) => Json(json!({
            "id": card["id"].as_str().unwrap_or_default(),
            "readme": card["readme"].as_str().unwrap_or_default(),
        }))
        .into_response(),
        None => failure(StatusCode::NOT_FOUND, "No description for this model."),
    }
}

/// Allow only the media operations used by the browser, on the configured loopback API.
async fn media(State(panel): State<Panel>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = parts.uri.path().replacen("/api/media/", "/v1/", 1);
    let video = video_route(parts.method.as_str(), &path);
    if parts.uri.query().is_some()
        || !(video
            || parts.method == Method::POST
                && matches!(
                    path.as_str(),
                    "/v1/images/generations" | IMAGE_EDITS_PATH | SPEECH_PATH | TRANSCRIPTIONS_PATH
                ))
    {
        return failure(StatusCode::NOT_FOUND, "Media route not available.");
    }
    let Some((listen, key)) = panel.gateway_address().await.zip(panel.key()) else {
        return failure(StatusCode::CONFLICT, "Start the connection first.");
    };
    let mut upstream = panel
        .stream
        .request(parts.method.clone(), format!("http://{listen}{path}"))
        .bearer_auth(key);
    if parts.method == Method::POST {
        let content_type = parts
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok());
        let content_type = if multipart_route(parts.method.as_str(), &path) {
            content_type.filter(|value| multipart_content_type(value))
        } else {
            content_type.filter(|value| value.split(';').next() == Some("application/json"))
        };
        let Some(content_type) = content_type else {
            return failure(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Unsupported media request format.",
            );
        };
        let upload = match panel.chat_uploads.reserve() {
            Ok(permit) => permit,
            Err(error) => return error.into_response(),
        };
        let body = match panel.chat_uploads.read(body).await {
            Ok(body) => body,
            Err(error) => return error.into_response(),
        };
        drop(upload);
        upstream = upstream.header("content-type", content_type).body(body);
    }
    gateway_response(upstream).await
}

async fn gateway_response(request: reqwest::RequestBuilder) -> Response {
    let response = request.send().await;
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
    let valid = serde_json::from_slice::<Value>(&body).ok().filter(|chat| {
        chat["messages"].is_array()
            && chat["title"]
                .as_str()
                .is_some_and(|title| title.chars().count() <= MAX_TITLE_CHARS)
    });
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
