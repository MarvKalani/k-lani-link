//! Host only: choose between `upstream` and the further `backends` by the request's model.
use crate::config::Config;
use anyhow::{Result, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashMap,
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
    /// Entries of backends whose privacy profile passed, primary first.
    listed: Vec<Value>,
    reachable: bool,
}

pub struct Routes {
    config: Config,
    /// May be bound to `upstream_socket`, so it only ever serves `upstream`.
    primary: reqwest::Client,
    network: reqwest::Client,
    catalog: Mutex<Option<Arc<Catalog>>>,
}

impl Routes {
    pub fn new(config: &Config) -> Result<Self> {
        let builder = || {
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
        };
        let primary = builder();
        #[cfg(unix)]
        let primary = match &config.upstream_socket {
            Some(path) => primary.unix_socket(path.clone()),
            None => primary,
        };
        Ok(Self {
            config: config.clone(),
            primary: primary.build()?,
            network: builder().build()?,
            catalog: Mutex::new(None),
        })
    }

    pub fn is_routed(&self) -> bool {
        !self.config.backends.is_empty()
    }

    pub fn target(&self, index: usize) -> Target<'_> {
        match index
            .checked_sub(1)
            .and_then(|i| self.config.backends.get(i))
        {
            Some(backend) => Target {
                http: &self.network,
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

    /// `GET /v1/models` across all backends whose privacy profile passed.
    pub async fn model_list(&self) -> Result<Vec<u8>> {
        let catalog = self.catalog().await;
        ensure!(catalog.reachable, "no backend answered the model list");
        Ok(serde_json::to_vec(
            &serde_json::json!({"object": "list", "data": catalog.listed}),
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
                let size = serde_json::to_vec(&model).map_or(usize::MAX, |entry| entry.len());
                if verified && bytes + size < MODEL_LIST_BYTES {
                    bytes += size + 1;
                    catalog.listed.push(model);
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
        let verified = !target.verify
            || crate::backend::verify_at(target.http, target.upstream, target.api_key)
                .await
                .is_ok();
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
