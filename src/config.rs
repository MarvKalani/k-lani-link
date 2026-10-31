use crate::diagnostic::Diagnostic;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;
use url::Url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayPolicy {
    #[default]
    Disabled,
    Allowed,
    Required,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum SignalingBackend {
    #[default]
    Mailbox,
    IceExchange,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Transport::is_webrtc")]
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https: Option<crate::mailbox::Options>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signaling_url: String,
    #[serde(default)]
    pub signaling_backend: SignalingBackend,
    #[serde(default)]
    pub signaling_token: String,
    #[serde(default)]
    pub room: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pair_key: String,
    #[serde(default)]
    pub pairing_state_dir: Option<PathBuf>,
    pub api_key: String,
    #[serde(default = "listen_default")]
    pub listen: SocketAddr,
    #[serde(default = "upstream_default")]
    pub upstream: String,
    #[serde(default)]
    pub upstream_socket: Option<PathBuf>,
    #[serde(default)]
    pub upstream_api_key: String,
    #[serde(default)]
    pub relay: RelayPolicy,
    #[serde(default)]
    pub ice_servers: Vec<IceServer>,
    #[serde(default = "request_timeout_default")]
    pub request_timeout_seconds: u64,
    #[serde(default = "concurrency_default")]
    pub max_concurrent_requests: usize,
    #[serde(default)]
    pub max_queued_requests: usize,
    /// Client only: a separate operator credential, never mounted in an AI application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management_token_file: Option<PathBuf>,
    /// Host only: explicitly enables management of an allowlisted local backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_control_socket: Option<PathBuf>,
    /// Host only. Forward prompts without first confirming the backend's privacy profile.
    /// The backend may then log or store them; `check` repeats this warning.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_unverified_backend: bool,
    /// Offer 127.0.0.1 as an ICE candidate. Only useful when both peers share one machine
    /// and no other network interface exists, e.g. offline tests.
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_loopback_candidate: bool,
    /// HTTPS only, set on both endpoints. After a lost session the host keeps each running
    /// response this many seconds and the gateway resumes it in the next session; 0 cancels
    /// at once. While detached the backend gets no credits, so at most one response window
    /// waits and generation is held back rather than run ahead.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub resume_seconds: u64,
    /// Host only: further OpenAI-compatible backends. A request goes to the first backend
    /// whose `/v1/models` lists its model; unlisted models go to `upstream` as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<Backend>,
}
/// Host only: a further backend reached through the same tunnel, chosen by model name.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Backend {
    pub upstream: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub upstream_api_key: String,
    /// Forward prompts without confirming this backend's privacy profile (e.g. LM Studio).
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_unverified_backend: bool,
    /// Allow a backend on another machine: a private IP address over plain HTTP (prompts
    /// then cross that network unencrypted) or any host over HTTPS.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_network_upstream: bool,
}
impl Backend {
    fn check(&self) -> Result<()> {
        let url = Url::parse(&self.upstream)?;
        ensure!(
            url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "backend upstream must be an origin, e.g. http://192.168.1.20:1234"
        );
        let allowed = loopback_url(&url) && matches!(url.scheme(), "http" | "https")
            || self.allow_network_upstream
                && match url.scheme() {
                    "https" => true,
                    "http" => private_address(&url),
                    _ => false,
                };
        ensure!(
            allowed,
            "backend upstream must be on loopback; with allow_network_upstream also a private IP address over http or any host over https"
        );
        Ok(())
    }
}
/// RFC 1918, link-local, unique-local IPv6 and 100.64.0.0/10 (shared address space, used
/// by Tailscale). A hostname never counts: DNS would decide where plain-text prompts go.
fn private_address(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_private() || ip.is_link_local() || a == 100 && b & 0xc0 == 64
        }
        Some(url::Host::Ipv6(ip)) => ip.is_unique_local() || ip.is_unicast_link_local(),
        _ => false,
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    #[default]
    Webrtc,
    Https,
}
impl Transport {
    pub fn is_webrtc(&self) -> bool {
        *self == Self::Webrtc
    }
}
fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero(value: &u64) -> bool {
    *value == 0
}
fn listen_default() -> SocketAddr {
    "127.0.0.1:8787".parse().unwrap()
}
fn upstream_default() -> String {
    "http://127.0.0.1:8080".into()
}
fn request_timeout_default() -> u64 {
    600
}
fn concurrency_default() -> usize {
    1
}

impl Config {
    /// Errors are fixed, secret-free texts: TOML parse errors can quote the offending value.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            Diagnostic::err(match error.kind() {
                std::io::ErrorKind::NotFound => "configuration file not found",
                std::io::ErrorKind::PermissionDenied => {
                    "configuration file is not readable by this user"
                }
                _ => "configuration file cannot be read",
            })
        })?;
        let mut cfg: Self =
            toml::from_str(&text).map_err(|error| Diagnostic::err(parse_problem(&text, &error)))?;
        if cfg.transport == Transport::Webrtc {
            cfg.room = k_lani_link_protocol::room_from_key(&cfg.pair_key).map_err(|_| {
                Diagnostic::err(
                    "pair_key must be a 43-character URL-safe base64 code (32 random bytes)",
                )
            })?;
        } else if let Some(options) = &mut cfg.https {
            let absolute = std::fs::canonicalize(path)
                .map_err(|_| Diagnostic::err("configuration path cannot be resolved"))?;
            options.resolve(
                absolute
                    .parent()
                    .ok_or_else(|| Diagnostic::err("invalid configuration path"))?,
            );
        }
        cfg.validate()?;
        Ok(cfg)
    }
    /// All messages below are static text or `url::ParseError`, never configuration values.
    pub fn validate(&self) -> Result<()> {
        self.check()
            .map_err(|error| Diagnostic::err(error.to_string()))
    }
    fn check(&self) -> Result<()> {
        ensure!(
            (1..=8).contains(&self.max_concurrent_requests),
            "max_concurrent_requests must be 1..8"
        );
        ensure!(
            self.max_queued_requests <= 16,
            "max_queued_requests must be 0..16"
        );
        if let Some(path) = &self.management_token_file {
            ensure!(path.is_absolute(), "management_token_file must be absolute");
        }
        if let Some(path) = &self.model_control_socket {
            ensure!(
                cfg!(unix) && path.is_absolute(),
                "model_control_socket requires Unix and an absolute path"
            );
            ensure!(
                self.upstream_socket.as_ref() != Some(path),
                "control and inference sockets must be separate"
            );
            ensure!(
                !self.allow_unverified_backend,
                "model management requires the verified privacy profile"
            );
        }
        if let Some(socket) = &self.upstream_socket {
            ensure!(
                cfg!(unix) && socket.is_absolute(),
                "upstream_socket requires Unix and an absolute path"
            );
        }
        ensure!(
            self.listen.ip().is_loopback(),
            "API listener must be a loopback address"
        );
        ensure!(
            self.api_key.len() >= 32
                && (self.transport == Transport::Https
                    || self.signaling_backend != SignalingBackend::Mailbox
                    || self.signaling_token.len() >= 32),
            "API key and mailbox access token must have at least 32 characters"
        );
        if self.transport == Transport::Webrtc {
            ensure!(
                self.https.is_none(),
                "HTTPS settings require transport = https"
            );
            ensure!(
                k_lani_link_protocol::valid_room(&self.room),
                "invalid room name"
            );
            ensure!(
                self.room
                    == k_lani_link_protocol::room_from_key(&self.pair_key)
                        .map_err(anyhow::Error::msg)?,
                "room must be derived from the connection code"
            );
            let signal = Url::parse(&self.signaling_url)?;
            ensure!(
                signal.scheme() == "https" || signal.scheme() == "http" && loopback_url(&signal),
                "signaling requires HTTPS except on loopback"
            );
            ensure!(
                signal.username().is_empty()
                    && signal.password().is_none()
                    && signal.query().is_none()
                    && signal.fragment().is_none(),
                "signaling URL must not contain credentials, query or fragment"
            );
        } else {
            ensure!(
                self.pair_key.is_empty()
                    && self.room.is_empty()
                    && self.signaling_url.is_empty()
                    && self.signaling_token.is_empty()
                    && self.ice_servers.is_empty()
                    && self.relay == RelayPolicy::Disabled
                    && !self.include_loopback_candidate,
                "HTTPS configuration must not contain a bootstrap code, signaling/STUN/TURN settings or loopback ICE"
            );
            self.https
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("HTTPS settings are required"))?
                .validate()?;
        }
        let upstream = Url::parse(&self.upstream)?;
        ensure!(
            matches!(upstream.scheme(), "http" | "https") && loopback_url(&upstream),
            "MVP upstream must be on loopback"
        );
        ensure!(
            upstream.path() == "/"
                && upstream.query().is_none()
                && upstream.fragment().is_none()
                && upstream.username().is_empty()
                && upstream.password().is_none(),
            "upstream must be an origin, e.g. http://127.0.0.1:8080"
        );
        ensure!(
            (10..=3600).contains(&self.request_timeout_seconds),
            "request timeout must be 10..3600 seconds"
        );
        ensure!(self.backends.len() <= 8, "at most 8 further backends");
        for backend in &self.backends {
            backend.check()?;
        }
        ensure!(
            self.resume_seconds == 0
                || self.transport == Transport::Https
                    && self.resume_seconds <= self.request_timeout_seconds,
            "resume_seconds needs the HTTPS transport and must not exceed request_timeout_seconds"
        );
        let mut has_turn = false;
        for server in &self.ice_servers {
            ensure!(
                !server.urls.is_empty(),
                "ICE server must contain at least one URL"
            );
            for uri in &server.urls {
                let scheme = uri.split(':').next().unwrap_or("");
                ensure!(
                    matches!(scheme, "stun" | "stuns" | "turn" | "turns"),
                    "unsupported ICE URI scheme"
                );
                if matches!(scheme, "turn" | "turns") {
                    has_turn = true;
                    ensure!(
                        !server.username.is_empty() && !server.credential.is_empty(),
                        "TURN needs explicit credentials"
                    );
                }
            }
        }
        match self.relay {
            RelayPolicy::Disabled if has_turn => {
                bail!("TURN URLs are forbidden while relay = disabled")
            }
            RelayPolicy::Required if !has_turn => {
                bail!("relay = required needs a configured TURN server")
            }
            _ => Ok(()),
        }
    }
}

fn parse_problem(text: &str, error: &toml::de::Error) -> String {
    let mut problem = String::from(
        "configuration file is not valid (check TOML syntax, field names and value types)",
    );
    if let Some(span) = error.span() {
        let end = span.start.min(text.len());
        let line = text.as_bytes()[..end]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1;
        problem.push_str(&format!("; see line {line}"));
    }
    // Missing fields are fixed schema names. Unknown field names are caller input
    // and can themselves contain a pasted credential, so do not echo them.
    let message = error.message();
    if message.starts_with("missing field") {
        problem.push_str(&format!(": {message}"));
    } else if message.starts_with("unknown field") {
        problem.push_str("; unknown field");
    }
    problem
}

/// True when group or others can access a secret-bearing file (Unix only).
pub fn loose_permissions(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o077 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

pub fn loopback_url(url: &Url) -> bool {
    matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        || matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
        || url.host_str() == Some("localhost")
}

pub fn token_matches(value: Option<&str>, expected: &str) -> bool {
    let expected = format!("Bearer {expected}");
    bool::from(value.unwrap_or("").as_bytes().ct_eq(expected.as_bytes()))
}

pub fn random_secret() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("OS randomness failed: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
pub fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("OS randomness failed: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn generate_pair(
    dir: &Path,
    signaling_url: &str,
    signaling_backend: SignalingBackend,
    stun: Vec<String>,
) -> Result<()> {
    // A non-recursive create refuses to overwrite an existing pair or follow an existing
    // symlink; the mode applies at creation, so the directory is never briefly open.
    let builder = &mut std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).context("create NEW pair directory")?;
    let code = random_secret()?;
    let cfg = Config {
        transport: Default::default(),
        https: None,
        signaling_url: signaling_url.into(),
        signaling_backend,
        signaling_token: random_secret()?,
        room: k_lani_link_protocol::room_from_key(&code).map_err(anyhow::Error::msg)?,
        pair_key: code,
        pairing_state_dir: None,
        api_key: random_secret()?,
        listen: listen_default(),
        upstream: upstream_default(),
        upstream_socket: None,
        upstream_api_key: String::new(),
        relay: RelayPolicy::Disabled,
        ice_servers: stun
            .into_iter()
            .map(|uri| IceServer {
                urls: vec![uri],
                ..Default::default()
            })
            .collect(),
        request_timeout_seconds: request_timeout_default(),
        allow_unverified_backend: false,
        include_loopback_candidate: false,
        resume_seconds: 0,
        backends: vec![],
        max_concurrent_requests: 1,
        max_queued_requests: 0,
        management_token_file: None,
        model_control_socket: None,
    };
    cfg.validate()?;
    let content = toml::to_string_pretty(&cfg)?;
    for file in ["host.toml", "client.toml"] {
        private_file(&dir.join(file), &content)?;
    }
    private_file(&dir.join("signaling-token.txt"), &cfg.signaling_token)?;
    private_file(&dir.join("pair-code.txt"), &cfg.pair_key)?;
    Ok(())
}
pub fn private_file(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(content.as_bytes())?;
    Ok(())
}

pub fn configure_endpoint(path: &Path, config: Config) -> Result<()> {
    config.validate()?;
    private_file(path, &toml::to_string_pretty(&config)?)
}

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn further_backends_leave_loopback_only_by_explicit_opt_in() {
        let check = |upstream: &str, network: bool| {
            Backend {
                upstream: upstream.into(),
                upstream_api_key: String::new(),
                allow_unverified_backend: true,
                allow_network_upstream: network,
            }
            .check()
            .is_ok()
        };
        for (upstream, plain, opted_in) in [
            ("http://127.0.0.1:1234", true, true),
            ("http://localhost:1234", true, true),
            ("http://192.168.1.20:1234", false, true),
            ("http://10.0.0.5:1234", false, true),
            ("http://100.101.102.103:1234", false, true),
            ("http://[fd00::5]:1234", false, true),
            ("http://203.0.113.7:1234", false, false),
            ("http://macmini.local:1234", false, false),
            ("https://models.example:443", false, true),
            ("http://192.168.1.20:1234/v1", false, false),
            ("http://user:pw@192.168.1.20:1234", false, false),
            ("ftp://192.168.1.20", false, false),
        ] {
            assert_eq!(check(upstream, false), plain, "{upstream} without opt-in");
            assert_eq!(check(upstream, true), opted_in, "{upstream} with opt-in");
        }
        let mut config = config();
        config.backends = vec![
            Backend {
                upstream: "http://127.0.0.1:1".into(),
                upstream_api_key: String::new(),
                allow_unverified_backend: false,
                allow_network_upstream: false,
            };
            9
        ];
        assert!(config.validate().is_err());
        config.backends.truncate(8);
        config.validate().unwrap();
    }

    use super::*;
    pub fn config() -> Config {
        Config {
            transport: Default::default(),
            https: None,
            signaling_url: "http://127.0.0.1:8788".into(),
            signaling_backend: SignalingBackend::Mailbox,
            signaling_token: "s".repeat(32),
            room: k_lani_link_protocol::room_from_key(&URL_SAFE_NO_PAD.encode([42; 32])).unwrap(),
            pair_key: URL_SAFE_NO_PAD.encode([42; 32]),
            pairing_state_dir: None,
            api_key: "a".repeat(32),
            listen: listen_default(),
            upstream: upstream_default(),
            upstream_socket: None,
            upstream_api_key: String::new(),
            relay: RelayPolicy::Disabled,
            ice_servers: vec![],
            request_timeout_seconds: 60,
            allow_unverified_backend: false,
            include_loopback_candidate: true,
            resume_seconds: 0,
            backends: vec![],
            max_concurrent_requests: 1,
            max_queued_requests: 0,
            management_token_file: None,
            model_control_socket: None,
        }
    }
    #[test]
    fn relay_is_explicit_and_no_servers_are_injected() {
        let mut cfg = config();
        assert!(cfg.validate().is_ok());
        assert!(cfg.ice_servers.is_empty());
        cfg.ice_servers.push(IceServer {
            urls: vec!["turn:relay.example:3478".into()],
            username: "user".into(),
            credential: "secret".into(),
        });
        assert!(cfg.validate().is_err());
        cfg.relay = RelayPolicy::Allowed;
        assert!(cfg.validate().is_ok());
        cfg.relay = RelayPolicy::Required;
        cfg.ice_servers.clear();
        assert!(cfg.validate().is_err());
    }
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("link-config-{name}-{}", random_id().unwrap()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }
    #[test]
    fn load_errors_name_the_problem_but_never_echo_values() {
        let dir = scratch("errors");
        let file = dir.join("c.toml");
        let valid = toml::to_string_pretty(&config()).unwrap();
        for (content, expected) in [
            (format!("{valid}\nunknown_option = 1\n"), "unknown field"),
            (
                valid.replace(
                    "request_timeout_seconds = 60",
                    "request_timeout_seconds = \"supersecretvalue\"",
                ),
                "see line",
            ),
            (
                "signaling_url = \"https://x.example\"\n".to_owned(),
                "missing field",
            ),
        ] {
            std::fs::write(&file, content).unwrap();
            let message = Config::load(&file).err().unwrap().to_string();
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("supersecretvalue") && !message.contains(&"a".repeat(32)));
        }
        assert_eq!(
            Config::load(&dir.join("absent.toml"))
                .err()
                .unwrap()
                .to_string(),
            "configuration file not found"
        );
        std::fs::write(
            &file,
            valid.replace("\"http://127.0.0.1:8788\"", "\"http://example.com\""),
        )
        .unwrap();
        assert!(
            Config::load(&file)
                .err()
                .unwrap()
                .to_string()
                .contains("HTTPS")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn detects_files_open_to_group_or_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let file = dir.join("c.toml");
        std::fs::write(&file, "x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(loose_permissions(&file));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(!loose_permissions(&file));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn privacy_opt_out_and_loopback_flag_are_off_unless_written_and_not_serialized() {
        let mut cfg = config();
        cfg.include_loopback_candidate = false;
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert!(!text.contains("allow_unverified_backend") && !text.contains("include_loopback"));
        let parsed: Config = toml::from_str(&text).unwrap();
        assert!(!parsed.allow_unverified_backend && !parsed.include_loopback_candidate);
        cfg.allow_unverified_backend = true;
        assert!(
            toml::to_string_pretty(&cfg)
                .unwrap()
                .contains("allow_unverified_backend = true")
        );
    }
    #[test]
    fn refuses_public_api_and_unencrypted_remote_signaling() {
        let mut cfg = config();
        cfg.listen = "0.0.0.0:8787".parse().unwrap();
        assert!(cfg.validate().is_err());
        cfg.listen = listen_default();
        cfg.signaling_url = "http://example.com".into();
        assert!(cfg.validate().is_err());
    }
}
