//! Host only: choose a backend by model or by a configured media route.
use crate::config::{Backend, Config, MODEL_CARD_README_BYTES, model_card_key};
use anyhow::{Result, ensure};
use k_lani_link_protocol::{IMAGE_EDITS_PATH, SPEECH_PATH, TRANSCRIPTIONS_PATH, video_route};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// How long a model catalog is reused, including for models no backend lists.
const CATALOG_SECONDS: u64 = 30;
/// The merged model list stays below the control panel's 64 KiB read limit.
const MODEL_LIST_BYTES: usize = 60 * 1024;

pub struct Target<'a> {
    pub http: &'a reqwest::Client,
    pub upstream: &'a str,
    pub api_key: &'a str,
    pub verify: bool,
}

struct Catalog {
    built: Instant,
    /// Model ID to backend index (0 is `upstream`); the first backend listing it wins.
    owner: HashMap<String, usize>,
    /// Entries of backends whose privacy profile passed, primary first, with their index.
    listed: Vec<(usize, Value)>,
    reachable: bool,
}

pub struct Routes {
    config: Config,
    /// May be bound to `upstream_socket`, so it only ever serves `upstream`.
    primary: reqwest::Client,
    backends: Vec<reqwest::Client>,
    catalog: Mutex<Option<Arc<Catalog>>>,
    /// With `load_on_demand`: the supervisor's last snapshot, kept current by the host.
    managed: Mutex<Option<crate::on_demand::Snapshot>>,
    /// The configured model cards as `GET /v1/models` carries them, by model ID.
    cards: HashMap<String, Value>,
    /// Their READMEs, read at start: key → (model ID, text).
    readmes: HashMap<String, (String, String)>,
}

impl Routes {
    pub fn new(config: &Config) -> Result<Self> {
        let client = |socket: Option<&Path>| {
            let builder = reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10));
            #[cfg(unix)]
            let builder = match socket {
                Some(path) => builder.unix_socket(path.to_owned()),
                None => builder,
            };
            #[cfg(not(unix))]
            let _ = socket;
            builder.build()
        };
        let mut cards = HashMap::new();
        let mut readmes = HashMap::new();
        for card in &config.model_cards {
            let mut shown = serde_json::Map::new();
            for (name, link) in [("page", &card.page), ("benchmarks", &card.benchmarks)] {
                if !link.is_empty() {
                    shown.insert(name.into(), Value::from(link.as_str()));
                }
            }
            if let Some(path) = &card.readme {
                let text = read_readme(path).map_err(|error| {
                    anyhow::anyhow!("model card readme for {}: {error}", card.id)
                })?;
                let key = model_card_key(&card.id);
                shown.insert("readme".into(), Value::from(key.as_str()));
                readmes.insert(key, (card.id.clone(), text));
            }
            cards.insert(card.id.clone(), Value::Object(shown));
        }
        Ok(Self {
            cards,
            readmes,
            config: config.clone(),
            primary: client(config.upstream_socket.as_deref())?,
            backends: config
                .backends
                .iter()
                .map(|backend| client(backend.upstream_socket.as_deref()))
                .collect::<Result<_, _>>()?,
            catalog: Mutex::new(None),
            managed: Mutex::new(None),
        })
    }

    pub fn is_routed(&self) -> bool {
        !self.config.backends.is_empty()
    }

    /// Media requests use the assigned service, including when it is unavailable.
    pub fn media_backend(&self, method: &str, path: &str) -> Option<usize> {
        let handles: fn(&Backend) -> bool = match (method, path) {
            ("POST", SPEECH_PATH) => |backend| backend.speech,
            ("POST", TRANSCRIPTIONS_PATH) => |backend| backend.transcriptions,
            ("POST", IMAGE_EDITS_PATH) => |backend| backend.image_edits,
            _ if video_route(method, path) => |backend| backend.video,
            _ => return None,
        };
        Some(
            self.config
                .backends
                .iter()
                .position(handles)
                .map_or(0, |index| index + 1),
        )
    }

    pub fn target(&self, index: usize) -> Target<'_> {
        match index
            .checked_sub(1)
            .and_then(|i| self.config.backends.get(i).zip(self.backends.get(i)))
        {
            Some((backend, http)) => Target {
                http,
                upstream: &backend.upstream,
                api_key: &backend.upstream_api_key,
                verify: !backend.allow_unverified_backend,
            },
            None => Target {
                http: &self.primary,
                upstream: &self.config.upstream,
                api_key: &self.config.upstream_api_key,
                verify: !self.config.allow_unverified_backend,
            },
        }
    }

    /// A model switch on the primary changes what it lists.
    pub fn invalidate(&self) {
        *self.catalog.lock().unwrap() = None;
    }

    pub fn set_managed(&self, snapshot: crate::on_demand::Snapshot) {
        *self.managed.lock().unwrap() = Some(snapshot);
        self.invalidate();
    }

    /// With `load_on_demand`, why a request for `upstream` must not reach it: llama.cpp
    /// answers with whatever model is loaded, so a request is never served by another one.
    pub fn managed_refusal(&self, index: usize, body: &[u8]) -> Option<(u16, &'static str)> {
        if !self.config.load_on_demand || index != 0 {
            return None;
        }
        let managed = self.managed.lock().unwrap();
        let (Some(snapshot), Some(model)) = (managed.as_ref(), crate::on_demand::requested(body))
        else {
            return None;
        };
        if !snapshot.lists(&model) {
            return Some((404, "this model is not offered by the model computer"));
        }
        (snapshot.active_model.as_deref() != Some(model.as_str()))
            .then_some((503, "this model is not loaded right now; try again"))
    }

    /// Whether `GET /v1/models` comes from the routes instead of a backend.
    pub fn lists_models(&self) -> bool {
        self.is_routed() || self.config.load_on_demand || !self.cards.is_empty()
    }

    /// A model entry with its configured card, which replaces one the backend sent.
    fn with_card(&self, mut model: Value) -> Value {
        if let Some(card) = model["id"].as_str().and_then(|id| self.cards.get(id)) {
            model["card"] = card.clone();
        }
        model
    }

    /// The README of the model card with this key, and the model it belongs to.
    pub fn readme(&self, key: &str) -> Option<&(String, String)> {
        self.readmes.get(key)
    }

    /// The privacy check before a request reaches backend `index`, if it needs one.
    pub async fn verify(&self, index: usize) -> Result<()> {
        let target = self.target(index);
        if !target.verify {
            return Ok(());
        }
        match (&self.config.model_control_socket, index) {
            (Some(socket), 0) => {
                crate::on_demand::verify(socket, target.http, target.upstream, target.api_key).await
            }
            _ => crate::backend::verify_at(target.http, target.upstream, target.api_key).await,
        }
    }

    /// The backend for a request body. Without further backends the body is not read.
    pub async fn choose(&self, body: &[u8]) -> usize {
        #[derive(Deserialize)]
        struct Request {
            model: Option<String>,
        }
        if !self.is_routed() {
            return 0;
        }
        let Some(model) = serde_json::from_slice::<Request>(body)
            .ok()
            .and_then(|request| request.model)
        else {
            return 0;
        };
        // An unlisted model goes to `upstream`, as without routing. A listed model never
        // falls back: its backend's own privacy check decides.
        self.catalog().await.owner.get(&model).copied().unwrap_or(0)
    }

    /// `GET /v1/models` across all backends whose privacy profile passed. With
    /// `load_on_demand` the primary contributes every allowlisted model, loaded or not.
    pub async fn model_list(&self) -> Result<Vec<u8>> {
        let mut managed = self
            .config
            .load_on_demand
            .then(|| self.managed.lock().unwrap().clone())
            .flatten();
        // A list asked for before the host's first look at the catalog asks for itself.
        if let (true, None, Some(socket)) = (
            self.config.load_on_demand,
            &managed,
            &self.config.model_control_socket,
        ) && let Ok(snapshot) = crate::on_demand::snapshot(socket).await
        {
            *self.managed.lock().unwrap() = Some(snapshot.clone());
            managed = Some(snapshot);
        }
        let mut data: Vec<Value> = managed
            .as_ref()
            .map(|s| s.entries())
            .unwrap_or_default()
            .into_iter()
            .map(|entry| self.with_card(entry))
            .collect();
        if self.is_routed() || managed.is_none() {
            let catalog = self.catalog().await;
            ensure!(
                catalog.reachable || managed.is_some(),
                "no backend answered the model list"
            );
            for (index, model) in &catalog.listed {
                let taken = data.iter().any(|entry| entry["id"] == model["id"]);
                if !(managed.is_some() && *index == 0) && !taken {
                    data.push(model.clone());
                }
            }
        }
        Ok(serde_json::to_vec(
            &serde_json::json!({"object": "list", "data": data}),
        )?)
    }

    async fn catalog(&self) -> Arc<Catalog> {
        if let Some(catalog) = self.catalog.lock().unwrap().as_ref()
            && catalog.built.elapsed() < Duration::from_secs(CATALOG_SECONDS)
        {
            return catalog.clone();
        }
        let lists = futures_util::future::join_all(
            (0..=self.config.backends.len()).map(|index| self.models_of(index)),
        )
        .await;
        let mut catalog = Catalog {
            built: Instant::now(),
            owner: HashMap::new(),
            listed: Vec::new(),
            reachable: false,
        };
        let mut bytes = 0;
        for (index, (models, verified)) in lists.into_iter().enumerate() {
            catalog.reachable |= models.is_some();
            for model in models.into_iter().flatten() {
                let Some(id) = model["id"].as_str() else {
                    continue;
                };
                if catalog.owner.contains_key(id) {
                    continue;
                }
                catalog.owner.insert(id.to_owned(), index);
                let model = self.with_card(model);
                let size = serde_json::to_vec(&model).map_or(usize::MAX, |entry| entry.len());
                if verified && bytes + size < MODEL_LIST_BYTES {
                    bytes += size + 1;
                    catalog.listed.push((index, model));
                }
            }
        }
        let catalog = Arc::new(catalog);
        *self.catalog.lock().unwrap() = Some(catalog.clone());
        catalog
    }

    /// The backend's model entries, and whether its privacy profile passed.
    async fn models_of(&self, index: usize) -> (Option<Vec<Value>>, bool) {
        let target = self.target(index);
        let verified = self.verify(index).await.is_ok();
        let mut request = target
            .http
            .get(format!(
                "{}/v1/models",
                target.upstream.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(5));
        if !target.api_key.is_empty() {
            request = request.bearer_auth(target.api_key);
        }
        let models = async {
            let mut response = request.send().await.ok()?.error_for_status().ok()?;
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if body.len() + chunk.len() > 1024 * 1024 {
                    return None;
                }
                body.extend_from_slice(&chunk);
            }
            let list: Value = serde_json::from_slice(&body).ok()?;
            list["data"].as_array().cloned()
        }
        .await;
        (models, verified)
    }
}

/// A model card's README: UTF-8 text of at most 32 KiB.
fn read_readme(path: &Path) -> Result<String> {
    let size = std::fs::metadata(path)?.len();
    ensure!(
        size <= MODEL_CARD_README_BYTES,
        "the file is larger than 32 KiB"
    );
    String::from_utf8(std::fs::read(path)?)
        .map_err(|_| anyhow::anyhow!("the file is not UTF-8 text"))
}
