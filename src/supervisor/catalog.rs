//! The operator's model catalog for the supervisor: which files may be loaded, under which
//! IDs and with which settings. Nothing in it reaches the supervisor from a client.
use anyhow::{Result, bail, ensure};
use serde_json::{Map, Number, Value};
use std::path::Path;

pub const MAX_CATALOG_BYTES: usize = 64 * 1024;
const LLAMA_KV: [&str; 3] = ["q8_0", "q4_0", "f16"];
const BASALT_ONLY: [&str; 7] = [
    "expert_profile",
    "memory_mode",
    "prefill_cpu_share",
    "pool_workers",
    "sampling",
    "answer_room",
    "fit_max_tokens",
];
const COMMON: [&str; 9] = [
    "id",
    "engine",
    "path",
    "context_tokens",
    "parallel",
    "slot_full_context",
    "prefill",
    "reasoning_budget_tokens",
    "kv",
];
const PREFILL_AUTO: [&str; 4] = ["auto", "auto:8192", "auto:16384", "auto:32768"];
/// Basalt's sampling defaults a catalog may set, with the values each accepts.
const SAMPLING: [&str; 6] = [
    "temperature",
    "top_p",
    "top_k",
    "min_p",
    "presence_penalty",
    "repetition_penalty",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Basalt,
    Llama,
}
impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Self::Basalt => "basalt",
            Self::Llama => "llama",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Prefill {
    Tokens(i64),
    Auto(String),
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub engine: Engine,
    pub path: String,
    pub context_tokens: i64,
    pub parallel: i64,
    pub slot_full_context: bool,
    pub prefill: Prefill,
    pub reasoning_budget_tokens: i64,
    /// llama.cpp only.
    pub kv: String,
    pub expert_profile: Option<String>,
    pub resident: bool,
    pub prefill_cpu_share: Option<Number>,
    pub pool_workers: Option<i64>,
    pub sampling: Vec<(String, Number)>,
    pub answer_room: Option<bool>,
    pub fit_max_tokens: bool,
}
impl Profile {
    /// The context each request may use: all of it with a shared or per-slot full context.
    pub fn context_per_slot(&self) -> i64 {
        if self.slot_full_context {
            self.context_tokens
        } else {
            self.context_tokens / self.parallel
        }
    }
}

pub struct Catalog {
    pub default: Option<String>,
    /// In the file's order, which the snapshot keeps.
    pub models: Vec<Profile>,
}
impl Catalog {
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.models.iter().find(|m| m.id == id)
    }
}

/// An operator alias such as `coder` or `Mellum2.1-12B-Q8_0`: never a path.
pub fn model_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    let first_ok = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    first_ok
        && value.len() <= 64
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

fn integer(value: &Value) -> Option<i64> {
    // JSON integers only: 4096.0 and true are refused, as before.
    if value.is_i64() || value.is_u64() {
        value.as_i64()
    } else {
        None
    }
}
fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite())
}
fn absolute_file(value: &str) -> bool {
    let path = Path::new(value);
    path.is_absolute() && path.is_file()
}

pub fn from_file(path: &Path) -> Result<Catalog> {
    let raw = std::fs::read(path)?;
    ensure!(raw.len() <= MAX_CATALOG_BYTES, "catalog too large");
    from_slice(&raw)
}

pub fn from_slice(raw: &[u8]) -> Result<Catalog> {
    let doc: Value = serde_json::from_slice(raw)?;
    let Some(doc) = doc.as_object() else {
        bail!("invalid catalog")
    };
    ensure!(
        doc.keys().all(|k| k == "default" || k == "models"),
        "invalid catalog"
    );
    let Some(rows) = doc.get("models").and_then(Value::as_array) else {
        bail!("invalid models")
    };
    ensure!((1..=32).contains(&rows.len()), "invalid models");
    let mut models: Vec<Profile> = Vec::new();
    for row in rows {
        let profile = profile(row)?;
        ensure!(
            models.iter().all(|m| m.id != profile.id),
            "invalid local model"
        );
        models.push(profile);
    }
    let default = match doc.get("default") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if models.iter().any(|m| &m.id == name) => Some(name.clone()),
        Some(_) => bail!("unknown default model"),
    };
    Ok(Catalog { default, models })
}

fn profile(row: &Value) -> Result<Profile> {
    let Some(row) = row.as_object() else {
        bail!("invalid model profile")
    };
    ensure!(
        row.keys()
            .all(|k| COMMON.contains(&k.as_str()) || BASALT_ONLY.contains(&k.as_str())),
        "invalid model profile"
    );
    let engine = match row.get("engine") {
        None => Engine::Basalt,
        Some(Value::String(name)) if name == "basalt" => Engine::Basalt,
        Some(Value::String(name)) if name == "llama" => Engine::Llama,
        Some(_) => bail!("invalid engine setting"),
    };
    let kv = match row.get("kv") {
        None => "q8_0".to_owned(),
        Some(_) if engine == Engine::Basalt => bail!("invalid engine setting"),
        Some(Value::String(kv)) if LLAMA_KV.contains(&kv.as_str()) => kv.clone(),
        Some(_) => bail!("invalid engine setting"),
    };
    let (Some(Value::String(id)), Some(Value::String(path))) = (row.get("id"), row.get("path"))
    else {
        bail!("invalid local model")
    };
    ensure!(model_id(id) && absolute_file(path), "invalid local model");
    if engine != Engine::Basalt {
        ensure!(
            !BASALT_ONLY.iter().any(|k| row.contains_key(*k)),
            "Basalt settings require the Basalt engine"
        );
    }
    let expert_profile = match row.get("expert_profile") {
        None => None,
        Some(Value::String(file)) if absolute_file(file) => Some(file.clone()),
        Some(_) => bail!("invalid Basalt expert profile"),
    };
    let mut slot_full_context = match row.get("slot_full_context") {
        None => false,
        Some(Value::Bool(full)) => *full,
        Some(_) => bail!("invalid slot setting"),
    };
    if engine == Engine::Llama {
        // One KV pool shared by all slots: each request may use the whole context.
        ensure!(
            path.ends_with(".gguf") && row.get("slot_full_context") != Some(&Value::Bool(false)),
            "invalid llama.cpp model"
        );
        slot_full_context = true;
    }
    let number = |key: &str, default: i64, low: i64, high: i64| -> Result<i64> {
        match row.get(key) {
            None => Ok(default),
            Some(value) => integer(value)
                .filter(|v| (low..=high).contains(v))
                .ok_or_else(|| anyhow::anyhow!("invalid model setting")),
        }
    };
    let context_tokens = number("context_tokens", 262_144, 4096, 1_048_576)?;
    let parallel = number("parallel", 1, 1, 8)?;
    let reasoning_budget_tokens = number(
        "reasoning_budget_tokens",
        2048,
        if engine == Engine::Llama { -1 } else { 0 },
        65_536,
    )?;
    let prefill = match row.get("prefill") {
        None => Prefill::Tokens(512),
        Some(Value::String(auto))
            if engine == Engine::Basalt && PREFILL_AUTO.contains(&auto.as_str()) =>
        {
            Prefill::Auto(auto.clone())
        }
        Some(value) => match integer(value).filter(|v| (128..=8192).contains(v)) {
            Some(tokens) => Prefill::Tokens(tokens),
            None => bail!("invalid prefill setting"),
        },
    };
    let flag = |key: &str| -> Result<Option<bool>> {
        match row.get(key) {
            None => Ok(None),
            Some(Value::Bool(value)) => Ok(Some(*value)),
            Some(_) => bail!("invalid Basalt token policy"),
        }
    };
    let answer_room = flag("answer_room")?;
    let fit_max_tokens = flag("fit_max_tokens")?.unwrap_or(false);
    let sampling = match row.get("sampling") {
        None => Vec::new(),
        Some(Value::Object(values)) => sampling(values)?,
        Some(_) => bail!("invalid Basalt sampling settings"),
    };
    let resident = match row.get("memory_mode") {
        None => false,
        Some(Value::String(mode)) if mode == "mmap" => false,
        Some(Value::String(mode)) if mode == "resident" => true,
        Some(_) => bail!("invalid expert memory mode"),
    };
    let prefill_cpu_share = match row.get("prefill_cpu_share") {
        None => None,
        Some(Value::Number(share))
            if finite(&Value::Number(share.clone())).is_some_and(|v| (0.0..=1.0).contains(&v)) =>
        {
            Some(share.clone())
        }
        Some(_) => bail!("invalid prefill CPU share"),
    };
    let pool_workers = match row.get("pool_workers") {
        None => None,
        Some(value) => Some(
            integer(value)
                .filter(|v| (1..=256).contains(v))
                .ok_or_else(|| anyhow::anyhow!("invalid CPU worker count"))?,
        ),
    };
    Ok(Profile {
        id: id.clone(),
        engine,
        path: path.clone(),
        context_tokens,
        parallel,
        slot_full_context,
        prefill,
        reasoning_budget_tokens,
        kv,
        expert_profile,
        resident,
        prefill_cpu_share,
        pool_workers,
        sampling,
        answer_room,
        fit_max_tokens,
    })
}

fn sampling(values: &Map<String, Value>) -> Result<Vec<(String, Number)>> {
    let mut out = Vec::new();
    for (key, value) in values {
        ensure!(
            SAMPLING.contains(&key.as_str()),
            "invalid Basalt sampling settings"
        );
        let (Value::Number(number), Some(v)) = (value, finite(value)) else {
            bail!("invalid Basalt sampling value")
        };
        let valid = match key.as_str() {
            "temperature" => v >= 0.0,
            "top_p" => v > 0.0 && v <= 1.0,
            "top_k" => integer(value).is_some_and(|k| k >= 0),
            "min_p" => (0.0..=1.0).contains(&v),
            "repetition_penalty" => v > 0.0,
            _ => true,
        };
        ensure!(valid, "invalid Basalt sampling value");
        out.push((key.clone(), number.clone()));
    }
    Ok(out)
}
