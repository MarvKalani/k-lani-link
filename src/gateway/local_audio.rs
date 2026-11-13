use crate::config::{LocalAudio, LocalAudioEndpoint};
use axum::{body::Body, http::StatusCode, response::Response};
use k_lani_link_protocol::{SPEECH_PATH, TRANSCRIPTIONS_PATH};
use serde_json::Value;
use std::time::Duration;
use tokio::{sync::OwnedSemaphorePermit, time::Instant};

const CATALOG_BYTES: usize = 64 * 1024;

pub(super) struct AudioRoutes {
    speech: Option<Endpoint>,
    transcriptions: Option<Endpoint>,
}

impl AudioRoutes {
    pub(super) fn new(config: &LocalAudio) -> anyhow::Result<Self> {
        Ok(Self {
            speech: config.speech.as_ref().map(Endpoint::new).transpose()?,
            transcriptions: config
                .transcriptions
                .as_ref()
                .map(Endpoint::new)
                .transpose()?,
        })
    }

    pub(super) fn endpoint(&self, method: &str, path: &str) -> Option<&Endpoint> {
        match (method, path) {
            ("POST", SPEECH_PATH) => self.speech.as_ref(),
            ("POST", TRANSCRIPTIONS_PATH) => self.transcriptions.as_ref(),
            _ => None,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.speech.is_some() || self.transcriptions.is_some()
    }

    pub(super) async fn models(&self) -> Vec<Value> {
        let results = futures_util::future::join_all(
            [
                (self.speech.as_ref(), "speech"),
                (self.transcriptions.as_ref(), "transcription"),
            ]
            .into_iter()
            .filter_map(|(endpoint, capability)| {
                endpoint.map(|endpoint| endpoint.models(capability))
            }),
        )
        .await;
        let mut models = Vec::new();
        for model in results.into_iter().flatten() {
            if models
                .iter()
                .all(|entry: &Value| entry["id"] != model["id"])
            {
                models.push(model);
            }
        }
        models
    }
}

pub(super) struct Endpoint {
    config: LocalAudioEndpoint,
    http: reqwest::Client,
}

impl Endpoint {
    fn new(config: &LocalAudioEndpoint) -> anyhow::Result<Self> {
        let builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5));
        #[cfg(unix)]
        let builder = match &config.upstream_socket {
            Some(path) => builder.unix_socket(path.clone()),
            None => builder,
        };
        Ok(Self {
            config: config.clone(),
            http: builder.build()?,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self.http.request(
            method,
            format!("{}{}", self.config.upstream.trim_end_matches('/'), path),
        );
        if self.config.upstream_api_key.is_empty() {
            request
        } else {
            request.bearer_auth(&self.config.upstream_api_key)
        }
    }

    async fn models(&self, capability: &'static str) -> Vec<Value> {
        let read = async {
            let mut response = self
                .request(reqwest::Method::GET, "/v1/models")
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len() + chunk.len() > CATALOG_BYTES {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes).ok()?;
            let models = value["data"].as_array()?;
            Some(
                models
                    .iter()
                    .filter_map(|model| {
                        let row = model.as_object()?;
                        row.get("id")?.as_str()?;
                        if row.get("capability").is_some_and(|kind| kind != capability) {
                            return None;
                        }
                        let mut row = row.clone();
                        row.insert("capability".into(), capability.into());
                        row.insert("local".into(), true.into());
                        Some(Value::Object(row))
                    })
                    .collect(),
            )
        };
        read.await.unwrap_or_default()
    }

    pub(super) async fn forward(
        &self,
        path: &str,
        content_type: &str,
        body: axum::body::Bytes,
        deadline: Instant,
        permit: OwnedSemaphorePermit,
    ) -> Response {
        if deadline <= Instant::now() {
            return super::error(StatusCode::GATEWAY_TIMEOUT, "local audio backend timed out");
        }
        let mut response = match self
            .request(reqwest::Method::POST, path)
            .header("content-type", content_type)
            .timeout(deadline.saturating_duration_since(Instant::now()))
            .body(body)
            .send()
            .await
        {
            Ok(response) if !response.status().is_redirection() => response,
            Err(error) if error.is_timeout() => {
                return super::error(StatusCode::GATEWAY_TIMEOUT, "local audio backend timed out");
            }
            _ => return super::error(StatusCode::BAD_GATEWAY, "local audio backend unavailable"),
        };
        let status = response.status();
        let mut builder = Response::builder()
            .status(status)
            .header("cache-control", "no-store");
        if let Some(content_type) = response.headers().get("content-type") {
            builder = builder.header("content-type", content_type);
        }
        let body = async_stream::try_stream! {
            let _permit = permit;
            while let Some(bytes) = response.chunk().await.map_err(std::io::Error::other)? {
                yield bytes;
            }
        };
        let body: std::pin::Pin<
            Box<dyn futures_util::Stream<Item = Result<_, std::io::Error>> + Send>,
        > = Box::pin(body);
        builder.body(Body::from_stream(body)).unwrap_or_else(|_| {
            super::error(StatusCode::BAD_GATEWAY, "invalid local audio response")
        })
    }
}
