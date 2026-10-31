//! Pairing by code: the host shows `472-1234-5678`, the client types it, and the client
//! folder that `init-https` would write arrives sealed over the relay.
//!
//! The first three digits name a relay slot, the other eight are the password of a SPAKE2
//! exchange. The relay sees only public SPAKE2 messages and ciphertext. A wrong guess costs
//! the slot: the client answers once, and the host closes the slot when the answer does not
//! prove the password. The host's state exists before the bundle leaves; the pairing counts
//! only after the client acknowledges its durable write.
use super::{
    Options,
    setup::{self, Material},
};
use crate::{chain_state::Lifetimes, diagnostic::Diagnostic};
use anyhow::Result;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use hkdf::Hkdf;
use k_lani_link_protocol::EndpointRole;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::Instant;

/// The relay a distribution was built for (`K_LANI_LINK_DEFAULT_RELAY` at build time).
/// Public source builds have none and ask for the relay address.
pub const DEFAULT_RELAY: Option<&str> = option_env!("K_LANI_LINK_DEFAULT_RELAY");
/// How long a shown code stays usable; the relay drops the slot at the same time.
pub const CODE_LIFETIME: Duration = Duration::from_secs(600);
const PASSWORD_DIGITS: usize = 8;
const MESSAGE_BYTES: usize = 33;
const CONFIRM: &[u8] = b"k-lani-link pairing: client knows the code";
const ACK: &[u8] = b"k-lani-link pairing: client state written";

/// A pairing code: relay slot and password. Displayed as `NNN-NNNN-NNNN`.
pub struct Code {
    slot: u16,
    password: String,
}
impl Code {
    fn new(slot: u16) -> Result<Self> {
        let mut password = String::with_capacity(PASSWORD_DIGITS);
        while password.len() < PASSWORD_DIGITS {
            let mut byte = [0u8; 1];
            getrandom::getrandom(&mut byte).map_err(|_| Diagnostic::err("OS randomness failed"))?;
            // Rejection sampling keeps every digit equally likely.
            if byte[0] < 250 {
                password.push(char::from(b'0' + byte[0] % 10));
            }
        }
        Ok(Self { slot, password })
    }
    /// Accepts the digits with any spaces, dashes or line ends in between.
    pub fn parse(text: &str) -> Result<Self> {
        let digits: String = text
            .chars()
            .filter(|c| !(c.is_whitespace() || *c == '-'))
            .collect();
        if digits.len() != 3 + PASSWORD_DIGITS || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Diagnostic::err(
                "A pairing code has eleven digits, e.g. 472-1234-5678.",
            ));
        }
        let slot: u16 = digits[..3].parse().unwrap_or(0);
        if !(100..=999).contains(&slot) {
            return Err(Diagnostic::err("This is not a valid pairing code."));
        }
        Ok(Self {
            slot,
            password: digits[3..].into(),
        })
    }
}
impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}",
            self.slot,
            &self.password[..4],
            &self.password[4..]
        )
    }
}

/// What the client receives: the shared material plus its own relay access.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    version: u8,
    material: Material,
    relay: String,
    admission_token: String,
    poll_wait_seconds: u64,
    http1_only: bool,
    http3: bool,
    post_writes: bool,
}

fn exchange(relay: &str, role: EndpointRole, password: &str) -> (Spake2<Ed25519Group>, Vec<u8>) {
    let password = Password::new(password.as_bytes());
    let host = Identity::new(format!("k-lani-link/pair/v1/host {relay}").as_bytes());
    let client = Identity::new(format!("k-lani-link/pair/v1/client {relay}").as_bytes());
    match role {
        EndpointRole::Host => Spake2::<Ed25519Group>::start_a(&password, &host, &client),
        EndpointRole::Client => Spake2::<Ed25519Group>::start_b(&password, &host, &client),
    }
}
/// Directional keys, bound to the relay, the slot and both public messages.
struct Keys {
    client: XChaCha20Poly1305,
    host: XChaCha20Poly1305,
    transcript: [u8; 32],
}
impl Keys {
    fn derive(
        shared: &[u8],
        relay: &str,
        slot: u16,
        opening: &[u8],
        answer: &[u8],
    ) -> Result<Self> {
        let mut hash = Sha256::new();
        hash.update(b"k-lani-link/pair/v1/transcript");
        for part in [relay.as_bytes(), &slot.to_be_bytes(), opening, answer] {
            hash.update((part.len() as u32).to_be_bytes());
            hash.update(part);
        }
        let transcript: [u8; 32] = hash.finalize().into();
        let kdf = Hkdf::<Sha256>::new(Some(&transcript), shared);
        let key = |label: &[u8]| -> Result<XChaCha20Poly1305> {
            let mut key = [0u8; 32];
            kdf.expand(label, &mut key)
                .map_err(|_| Diagnostic::err("pairing key derivation failed"))?;
            XChaCha20Poly1305::new_from_slice(&key)
                .map_err(|_| Diagnostic::err("pairing key derivation failed"))
        };
        Ok(Self {
            client: key(b"k-lani-link/pair/v1/client-to-host")?,
            host: key(b"k-lani-link/pair/v1/host-to-client")?,
            transcript,
        })
    }
    fn seal(&self, cipher: &XChaCha20Poly1305, counter: u8, message: &[u8]) -> Result<Vec<u8>> {
        cipher
            .encrypt(
                &nonce(counter),
                Payload {
                    msg: message,
                    aad: &self.transcript,
                },
            )
            .map_err(|_| Diagnostic::err("pairing encryption failed"))
    }
    fn open(&self, cipher: &XChaCha20Poly1305, counter: u8, sealed: &[u8]) -> Option<Vec<u8>> {
        cipher
            .decrypt(
                &nonce(counter),
                Payload {
                    msg: sealed,
                    aad: &self.transcript,
                },
            )
            .ok()
    }
}
fn nonce(counter: u8) -> XNonce {
    let mut nonce = [0u8; 24];
    nonce[23] = counter;
    nonce.into()
}

/// One spelling of a relay address for both sides: lowercase host, no default port, no
/// trailing slash. The keys are bound to it, so `Relay.example.com:443/` must not differ.
pub fn canonical(relay: &str) -> Result<String> {
    let url =
        url::Url::parse(relay.trim()).map_err(|_| Diagnostic::err("invalid HTTPS relay URL"))?;
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// Talks to the slot routes of one relay; writes always use POST, which every relay accepts.
struct Slots {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}
enum Read {
    Found(Vec<u8>),
    Empty,
    Missing,
}
impl Slots {
    fn new(
        relay: &str,
        proxy: Option<&str>,
        ca: Option<&[u8]>,
        token: Option<String>,
    ) -> Result<Self> {
        let mut http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(40));
        if let Some(proxy) = proxy {
            http = http.proxy(
                reqwest::Proxy::all(proxy)
                    .map_err(|_| Diagnostic::err("invalid explicit HTTPS proxy configuration"))?,
            );
        }
        if let Some(pem) = ca {
            for root in reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|_| Diagnostic::err("invalid HTTPS CA certificate"))?
            {
                http = http.add_root_certificate(root);
            }
        }
        Ok(Self {
            http: http
                .build()
                .map_err(|_| Diagnostic::err("HTTPS client configuration failed"))?,
            base: relay.trim_end_matches('/').to_owned(),
            token,
        })
    }
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self.http.request(method, format!("{}{path}", self.base));
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
    async fn open(&self, opening: &[u8]) -> Result<u16> {
        let response = self
            .request(reqwest::Method::POST, "/v1/pair")
            .body(opening.to_vec())
            .send()
            .await
            .map_err(|_| Diagnostic::err(NETWORK))?;
        match response.status().as_u16() {
            201 => {}
            401 => return Err(Diagnostic::err(REFUSED)),
            403 => {
                return Err(Diagnostic::err(
                    "Pairing by code is turned off on this relay for this token (pair_slots = 0). Copy the client folder from init-https instead.",
                ));
            }
            404 | 405 => return Err(Diagnostic::err(UNSUPPORTED)),
            429 => {
                return Err(Diagnostic::err(
                    "The relay holds too many open pairing codes. Wait ten minutes or use an open one.",
                ));
            }
            _ => return Err(Diagnostic::err(NETWORK)),
        }
        let text = response
            .text()
            .await
            .map_err(|_| Diagnostic::err(NETWORK))?;
        text.trim()
            .parse::<u16>()
            .ok()
            .filter(|n| (100..=999).contains(n))
            .ok_or_else(|| Diagnostic::err(UNSUPPORTED))
    }
    async fn read(&self, slot: u16, part: &str, wait: u64) -> Result<Read> {
        let response = self
            .request(
                reqwest::Method::GET,
                &format!("/v1/pair/{slot}/{part}?wait={wait}"),
            )
            .send()
            .await
            .map_err(|_| Diagnostic::err(NETWORK))?;
        match response.status().as_u16() {
            200 => Ok(Read::Found(
                response
                    .bytes()
                    .await
                    .map_err(|_| Diagnostic::err(NETWORK))?
                    .to_vec(),
            )),
            204 => Ok(Read::Empty),
            404 => Ok(Read::Missing),
            401 => Err(Diagnostic::err(REFUSED)),
            429 => Err(Diagnostic::err(BRAKED)),
            _ => Err(Diagnostic::err(NETWORK)),
        }
    }
    /// False: the slot does not exist (any more) or the part is already written.
    async fn write(&self, slot: u16, part: &str, body: Vec<u8>) -> Result<bool> {
        let response = self
            .request(reqwest::Method::POST, &format!("/v1/pair/{slot}/{part}"))
            .body(body)
            .send()
            .await
            .map_err(|_| Diagnostic::err(NETWORK))?;
        match response.status().as_u16() {
            204 => Ok(true),
            404 => Ok(false),
            401 => Err(Diagnostic::err(REFUSED)),
            429 => Err(Diagnostic::err(BRAKED)),
            _ => Err(Diagnostic::err(NETWORK)),
        }
    }
    async fn close(&self, slot: u16) {
        let _ = self
            .request(reqwest::Method::DELETE, &format!("/v1/pair/{slot}"))
            .send()
            .await;
    }
    /// Whether this relay offers pairing at all: its `/v1/pair` refuses GET with 405.
    async fn supported(&self) -> bool {
        self.http
            .get(format!("{}/v1/pair", self.base))
            .send()
            .await
            .is_ok_and(|r| r.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED)
    }
}

const NETWORK: &str = "The relay cannot be reached. Check the relay address and the network.";
const REFUSED: &str =
    "The relay refused the admission token: it is wrong, expired or limited to other addresses.";
const UNSUPPORTED: &str =
    "This relay cannot pair by code. Update it, or copy the client folder from init-https.";
const BRAKED: &str =
    "Too many failed attempts from this address. Wait a few minutes and try again.";

/// Removes a half-made pairing folder unless the pairing completed.
struct Unfinished(Option<PathBuf>);
impl Unfinished {
    fn done(mut self) {
        self.0 = None;
    }
}
impl Drop for Unfinished {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Host side. Writes `dir/host/host.toml`, calls `show` with the code and returns once the
/// client has acknowledged its folder. On any failure the new directory is removed again.
pub async fn offer(
    dir: &Path,
    options: Options,
    client_token: &str,
    limits: Lifetimes,
    upstream: &setup::Upstream,
    show: impl FnOnce(&str),
) -> Result<()> {
    options.validate()?;
    let mut options = options;
    let relay = canonical(&options.relays[0])?;
    options.relays = vec![relay.clone()];
    let host_token = super::secret(&options.admission_token_file)?;
    let certificate = setup::certificate(&options)?;
    let slots = Slots::new(
        &relay,
        options.proxy_url.as_deref(),
        certificate.as_deref(),
        Some(host_token.clone()),
    )?;
    let dir = setup::new_directory(dir)?;
    let unfinished = Unfinished(Some(dir.clone()));
    let material = Material::new(limits)?;
    let folder = dir.join("host");
    setup::initialize(&folder, EndpointRole::Host, &material)?;
    setup::export(
        &folder,
        "host",
        &material,
        &options,
        &host_token,
        certificate.as_deref(),
        Some(upstream),
    )?;
    // The slot number is part of the code, so the password is drawn once it is known.
    let mut code = Code::new(100)?;
    let (state, opening) = exchange(&relay, EndpointRole::Host, &code.password);
    code.slot = slots.open(&opening).await?;
    let slot = code.slot;
    show(&code.to_string());
    let deadline = Instant::now() + CODE_LIFETIME;
    let result = async {
        let answer = wait(&slots, slot, "b", deadline).await?;
        let (message, confirm) = answer.split_at(MESSAGE_BYTES.min(answer.len()));
        let keys = state
            .finish(message)
            .ok()
            .map(|shared| Keys::derive(&shared, &relay, slot, &opening, message))
            .transpose()?
            .filter(|keys| keys.open(&keys.client, 0, confirm).as_deref() == Some(CONFIRM))
            .ok_or_else(|| {
                Diagnostic::err(
                    "The code was entered wrongly on the other computer. Create a new code.",
                )
            })?;
        let bundle = Bundle {
            version: 1,
            material,
            relay: relay.clone(),
            admission_token: client_token.to_owned(),
            poll_wait_seconds: options.poll_wait_seconds,
            http1_only: options.http1_only,
            http3: options.http3,
            post_writes: options.post_writes,
        };
        let sealed = keys.seal(&keys.host, 0, &serde_json::to_vec(&bundle)?)?;
        if !slots.write(slot, "c", sealed).await? {
            return Err(Diagnostic::err(EXPIRED));
        }
        let ack = wait(&slots, slot, "d", deadline).await?;
        if keys.open(&keys.client, 1, &ack).as_deref() != Some(ACK) {
            return Err(Diagnostic::err(
                "The other computer did not confirm the pairing. Create a new code.",
            ));
        }
        // Checked: now nothing in the slot is needed any more.
        slots.close(slot).await;
        Ok(())
    }
    .await;
    if result.is_err() {
        slots.close(slot).await;
    }
    result?;
    unfinished.done();
    Ok(())
}
const EXPIRED: &str = "The pairing code expired or was already used. Create a new code.";

/// Host-side long poll for one part until it arrives or the code expires.
async fn wait(slots: &Slots, slot: u16, part: &str, deadline: Instant) -> Result<Vec<u8>> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now()).as_secs();
        if left == 0 {
            return Err(Diagnostic::err(EXPIRED));
        }
        match slots.read(slot, part, left.min(super::LONG_POLL)).await {
            Ok(Read::Found(bytes)) => return Ok(bytes),
            Ok(Read::Empty) => {}
            Ok(Read::Missing) => return Err(Diagnostic::err(EXPIRED)),
            // A dropped long poll is retried until the code expires.
            Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
}

/// Client side: what the joining computer supplies itself.
pub struct Join<'a> {
    pub relay: &'a str,
    pub proxy_url: Option<String>,
    pub ca_cert_file: Option<PathBuf>,
}
/// Writes a new client folder `dir` with `client.toml` and returns its path.
pub async fn join(dir: &Path, code: &Code, local: Join<'_>) -> Result<PathBuf> {
    let relay = canonical(local.relay)?;
    // Validate the relay address and local transport choices before contacting anything.
    let check = Options {
        relays: vec![relay.clone()],
        state_dir: std::env::temp_dir(),
        admission_token_file: std::env::temp_dir(),
        proxy_url: local.proxy_url.clone(),
        ca_cert_file: local.ca_cert_file.clone(),
        poll_wait_seconds: super::LONG_POLL,
        http1_only: false,
        http3: false,
        post_writes: false,
    };
    check.validate()?;
    let certificate = setup::certificate(&check)?;
    let slots = Slots::new(
        &relay,
        local.proxy_url.as_deref(),
        certificate.as_deref(),
        None,
    )?;
    let opening = match slots.read(code.slot, "a", 0).await? {
        Read::Found(bytes) => bytes,
        _ if !slots.supported().await => return Err(Diagnostic::err(UNSUPPORTED)),
        _ => {
            return Err(Diagnostic::err(
                "This code is unknown, expired or already used. Ask for a new code.",
            ));
        }
    };
    let (state, message) = exchange(&relay, EndpointRole::Client, &code.password);
    let shared = state
        .finish(&opening)
        .map_err(|_| Diagnostic::err("The relay returned an invalid pairing message."))?;
    let keys = Keys::derive(&shared, &relay, code.slot, &opening, &message)?;
    let mut answer = message.clone();
    answer.extend(keys.seal(&keys.client, 0, CONFIRM)?);
    if !slots.write(code.slot, "b", answer).await? {
        return Err(Diagnostic::err(EXPIRED));
    }
    let deadline = Instant::now() + CODE_LIFETIME;
    let sealed = loop {
        if Instant::now() >= deadline {
            return Err(Diagnostic::err(EXPIRED));
        }
        match slots.read(code.slot, "c", 0).await {
            Ok(Read::Found(bytes)) => break bytes,
            Ok(Read::Empty) | Err(_) => tokio::time::sleep(Duration::from_secs(1)).await,
            // The host closes the slot when the answer did not prove the code.
            Ok(Read::Missing) => {
                return Err(Diagnostic::err(
                    "The code was not accepted. Check the digits and ask for a new code.",
                ));
            }
        }
    };
    let bundle: Bundle = keys
        .open(&keys.host, 0, &sealed)
        .and_then(|plain| serde_json::from_slice(&plain).ok())
        .filter(|bundle: &Bundle| bundle.version == 1 && bundle.relay == relay)
        .ok_or_else(|| {
            Diagnostic::err("The other side could not be verified. Ask for a new code.")
        })?;
    let dir = setup::new_directory(dir)?;
    let unfinished = Unfinished(Some(dir.clone()));
    let options = Options {
        relays: vec![relay],
        state_dir: dir.join("state"),
        admission_token_file: dir.join("relay-token"),
        proxy_url: local.proxy_url,
        ca_cert_file: local.ca_cert_file,
        poll_wait_seconds: bundle.poll_wait_seconds,
        http1_only: bundle.http1_only,
        http3: bundle.http3 && check.proxy_url.is_none(),
        post_writes: bundle.post_writes,
    };
    setup::initialize(&dir, EndpointRole::Client, &bundle.material)?;
    setup::export(
        &dir,
        "client",
        &bundle.material,
        &options,
        &bundle.admission_token,
        certificate.as_deref(),
        None,
    )?;
    if !slots
        .write(code.slot, "d", keys.seal(&keys.client, 1, ACK)?)
        .await?
    {
        return Err(Diagnostic::err(EXPIRED));
    }
    unfinished.done();
    Ok(dir.join("client.toml"))
}

#[cfg(test)]
pub(crate) mod tests;
