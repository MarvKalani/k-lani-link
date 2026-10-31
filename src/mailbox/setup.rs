//! Explicit issuance; existing or partial state is never bootstrapped on startup.
use super::{Options, secret};
use crate::{
    chain_state::{Lifetimes, Store},
    config::{Config, RelayPolicy, SignalingBackend, Transport, private_file, random_secret},
    diagnostic::Diagnostic,
};
use anyhow::Result;
use k_lani_link_protocol::{Chain, EndpointRole};
use std::path::{Path, PathBuf};

pub fn config(options: Options, api_key: String) -> Config {
    Config {
        transport: Transport::Https,
        https: Some(options),
        signaling_url: String::new(),
        signaling_backend: SignalingBackend::Mailbox,
        signaling_token: String::new(),
        room: String::new(),
        pair_key: String::new(),
        pairing_state_dir: None,
        api_key,
        listen: "127.0.0.1:8787".parse().unwrap(),
        upstream: "http://127.0.0.1:8080".into(),
        upstream_socket: None,
        upstream_api_key: String::new(),
        relay: RelayPolicy::Disabled,
        ice_servers: Vec::new(),
        request_timeout_seconds: 600,
        max_concurrent_requests: 1,
        max_queued_requests: 0,
        management_token_file: None,
        model_control_socket: None,
        allow_unverified_backend: false,
        include_loopback_candidate: false,
        resume_seconds: 0,
        backends: vec![],
    }
}
/// Where the host forwards requests. Only the host folder records it.
#[derive(Clone, Debug)]
pub struct Upstream {
    pub url: String,
    /// A Unix socket instead of TCP, e.g. a backend container without a network.
    pub socket: Option<PathBuf>,
}
impl Default for Upstream {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:8080".into(),
            socket: None,
        }
    }
}
/// What both endpoints of one pairing share: the initial chain code, the issue time, the
/// lifetimes and the local API key. Pairing by code carries exactly this to the client.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub initial: String,
    pub issued: u64,
    pub limits: Lifetimes,
    pub api_key: String,
}
impl Material {
    pub fn new(limits: Lifetimes) -> Result<Self> {
        Ok(Self {
            initial: random_secret()?,
            issued: crate::signal::now(),
            limits,
            api_key: random_secret()?,
        })
    }
}
/// A new private directory (never an existing one), resolved to its absolute path.
pub fn new_directory(dir: &Path) -> Result<PathBuf> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|_| Diagnostic::err("HTTPS issuance requires a new private directory"))?;
    std::fs::canonicalize(dir)
        .map_err(|_| Diagnostic::err("HTTPS issuance directory cannot be resolved"))
}
/// Initializes one endpoint's durable chain state in `folder/state`.
pub fn initialize(folder: &Path, role: EndpointRole, material: &Material) -> Result<()> {
    let initial = Chain::from_code(&material.initial)
        .map_err(|_| Diagnostic::err("HTTPS initial credential could not be generated"))?;
    crate::pairing::private_directory(folder)?;
    let mut store = Store::open(&folder.join("state"), role)?;
    store.initialize(&initial, material.issued, material.limits)
}
/// Writes the portable configuration `folder/<name>.toml` beside its token and optional CA.
pub fn export(
    folder: &Path,
    name: &str,
    material: &Material,
    options: &Options,
    token: &str,
    certificate: Option<&[u8]>,
    upstream: Option<&Upstream>,
) -> Result<()> {
    private_file(&folder.join("relay-token"), token)?;
    if let Some(data) = certificate {
        private_file(
            &folder.join("ca.pem"),
            std::str::from_utf8(data).map_err(|_| Diagnostic::err("HTTPS CA must use PEM text"))?,
        )?;
    }
    let mut selected = options.clone();
    selected.state_dir = folder.join("state");
    selected.admission_token_file = folder.join("relay-token");
    selected.ca_cert_file = certificate.map(|_| folder.join("ca.pem"));
    let mut endpoint = config(selected, material.api_key.clone());
    if let Some(upstream) = upstream {
        endpoint.upstream.clone_from(&upstream.url);
        endpoint.upstream_socket.clone_from(&upstream.socket);
    }
    endpoint.validate()?;
    let selected = endpoint.https.as_mut().unwrap();
    selected.state_dir = "state".into();
    selected.admission_token_file = "relay-token".into();
    selected.ca_cert_file = certificate.map(|_| "ca.pem".into());
    private_file(
        &folder.join(format!("{name}.toml")),
        &toml::to_string_pretty(&endpoint)?,
    )
}
/// Validates an optional public CA bundle before any credential is created.
pub fn certificate(options: &Options) -> Result<Option<Vec<u8>>> {
    let Some(path) = &options.ca_cert_file else {
        return Ok(None);
    };
    let data = super::public_file(path, 128 * 1024)?;
    if data.len() > 128 * 1024
        || reqwest::Certificate::from_pem_bundle(&data).map_or(true, |roots| roots.is_empty())
    {
        return Err(Diagnostic::err("invalid or oversized HTTPS CA certificate"));
    }
    Ok(Some(data))
}
/// `client_token`: a separate admission token for the client folder (one token per device,
/// both in one relay space). Without it, both folders get the same token.
pub fn issue(
    dir: &Path,
    options: Options,
    client_token: Option<&Path>,
    limits: Lifetimes,
    upstream: &Upstream,
) -> Result<()> {
    options.validate()?;
    let host_token = secret(&options.admission_token_file)?;
    let client_token = match client_token {
        Some(path) => secret(path)?,
        None => host_token.clone(),
    };
    let certificate = certificate(&options)?;
    let dir = new_directory(dir)?;
    let material = Material::new(limits)?;
    for (name, role) in [
        ("host", EndpointRole::Host),
        ("client", EndpointRole::Client),
    ] {
        initialize(&dir.join(name), role, &material)?;
    }
    // Only after BOTH durable files exist may either portable configuration be exported.
    for (name, token) in [("host", &host_token), ("client", &client_token)] {
        export(
            &dir.join(name),
            name,
            &material,
            &options,
            token,
            certificate.as_deref(),
            (name == "host").then_some(upstream),
        )?;
    }
    Ok(())
}
