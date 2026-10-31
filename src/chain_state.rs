//! Private, fail-closed credential storage. No box, transport, CLI or configuration wiring.
//! The contract and crash ordering are specified in docs/DESIGN-MAILBOX.md.
use crate::{diagnostic::Diagnostic, pairing::private_directory};
use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use k_lani_link_protocol::{
    Accepted, Chain, EndpointRole, Established, HostChains, MAX_PENDING, MAX_RETIRED,
    PENDING_BYTES, Pending, RETIRED_GENERATION_BYTES, RetiredGeneration, Verdict, Via,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const HEADER: &str = "k-lani-link chain v2\n";
const MAX_FILE_BYTES: u64 = 8192;
pub const DAMAGED: &str = "the pairing state file is damaged or unsupported; do not edit it or restore the bootstrap code";
pub const MISSING: &str = "the pairing state file is missing; issue a new pair explicitly";
pub const REVOKED: &str = "this pairing state is revoked; issue a new pair";
pub const EXPIRED: &str = "this pairing state has expired; issue a new pair";
pub const IO_FAILURE: &str =
    "pairing state persistence failed; stop the session and reopen the state";
const CLOCK: &str = "the clock precedes the last saved pairing activity";
const EXISTS: &str = "pairing state already exists; initialization will not replace it";
const POLICY: &str = "invalid pairing lifetime policy";
const ROLE: &str = "pairing state operation has the wrong endpoint role";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lifetimes {
    pub unused_seconds: u64,
    pub idle_seconds: u64,
    pub absolute_seconds: Option<u64>,
}
impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            unused_seconds: 86_400,
            idle_seconds: 30 * 86_400,
            absolute_seconds: None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lifecycle {
    issued_at: u64,
    last_seen: u64,
    last_activity: u64,
    paired: bool,
    limits: Lifetimes,
}
impl Lifecycle {
    fn new(now: u64, limits: Lifetimes) -> Result<Self> {
        let value = Self {
            issued_at: now,
            last_seen: now,
            last_activity: now,
            paired: false,
            limits,
        };
        value.validate().map_err(|_| Diagnostic::err(POLICY))?;
        Ok(value)
    }
    fn validate(&self) -> Result<()> {
        if self.limits.unused_seconds == 0
            || self.limits.idle_seconds == 0
            || self.limits.absolute_seconds == Some(0)
            || self.last_activity < self.issued_at
            || self.last_seen < self.last_activity
            || (!self.paired && self.last_activity != self.issued_at)
            || self
                .issued_at
                .checked_add(self.limits.unused_seconds)
                .is_none()
            || self
                .last_activity
                .checked_add(self.limits.idle_seconds)
                .is_none()
            || self
                .limits
                .absolute_seconds
                .is_some_and(|n| self.issued_at.checked_add(n).is_none())
        {
            return Err(Diagnostic::err(DAMAGED));
        }
        Ok(())
    }
    pub fn issued_at(&self) -> u64 {
        self.issued_at
    }
    pub fn paired(&self) -> bool {
        self.paired
    }
    pub fn last_activity(&self) -> u64 {
        self.last_activity
    }
    pub fn last_seen(&self) -> u64 {
        self.last_seen
    }
    pub fn expires_at(&self) -> u64 {
        let deadline = if self.paired {
            self.last_activity.saturating_add(self.limits.idle_seconds)
        } else {
            self.issued_at.saturating_add(self.limits.unused_seconds)
        };
        self.limits.absolute_seconds.map_or(deadline, |age| {
            deadline.min(self.issued_at.saturating_add(age))
        })
    }
    fn check(&self, now: u64) -> Result<()> {
        self.validate()?;
        if now < self.last_seen {
            return Err(Diagnostic::err(CLOCK));
        }
        if now >= self.expires_at() {
            return Err(Diagnostic::err(EXPIRED));
        }
        Ok(())
    }
    fn observe(&mut self, now: u64, paired_activity: bool) -> Result<()> {
        self.check(now)?;
        self.last_seen = now;
        if paired_activity {
            self.paired = true;
            self.last_activity = now;
        }
        self.validate()
    }
}
#[derive(Clone)]
pub struct HostState {
    pub chains: HostChains,
    pub lifecycle: Lifecycle,
}
#[derive(Clone)]
pub struct ClientState {
    pub chain: Chain,
    pub lifecycle: Lifecycle,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Host,
    Client,
}
impl From<EndpointRole> for Role {
    fn from(role: EndpointRole) -> Self {
        match role {
            EndpointRole::Host => Self::Host,
            EndpointRole::Client => Self::Client,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Disk {
    Host {
        lifecycle: Lifecycle,
        cur: String,
        pending: Vec<String>,
        retired: Vec<String>,
    },
    Client {
        lifecycle: Lifecycle,
        cur: String,
    },
    Revoked {
        role: Role,
    },
}
impl Disk {
    fn role(&self) -> Role {
        match self {
            Self::Host { .. } => Role::Host,
            Self::Client { .. } => Role::Client,
            Self::Revoked { role } => *role,
        }
    }
}
fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
fn decode<const N: usize>(text: &str) -> Result<[u8; N]> {
    URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| Diagnostic::err(DAMAGED))
}
fn host_disk(state: &HostState) -> Disk {
    Disk::Host {
        lifecycle: state.lifecycle,
        cur: encode(state.chains.current().as_bytes()),
        pending: state
            .chains
            .pending()
            .iter()
            .map(|p| encode(&p.to_bytes()))
            .collect(),
        retired: state
            .chains
            .retired()
            .iter()
            .map(|r| encode(&r.to_bytes()))
            .collect(),
    }
}

/// Exclusive state owner. Mutations require &mut self; a failed write poisons this instance.
/// Revocation requires stopping live sessions first: disk state cannot erase outside cipher objects.
pub struct Store {
    dir: PathBuf,
    file: &'static str,
    role: Role,
    poisoned: bool,
    _lock: File,
}
impl Drop for Store {
    fn drop(&mut self) {
        // Closing one descriptor need not release a flock if a concurrently spawned
        // child still holds a duplicate before exec. End ownership explicitly.
        let _ = self._lock.unlock();
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    BeforeWrite,
    Written,
    Synced,
    Renamed,
    DirectorySynced,
}
#[cfg(test)]
thread_local! {
    static CRASH: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
    static EXIT_AT: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
fn step(at: Step) -> std::io::Result<()> {
    if EXIT_AT.with(|selected| selected.get()) == Some(at) {
        std::process::exit(91);
    }
    if CRASH.with(|selected| selected.get()) == Some(at) {
        return Err(std::io::Error::other("injected persistence failure"));
    }
    Ok(())
}
#[cfg(not(test))]
fn step(_: Step) -> std::io::Result<()> {
    Ok(())
}

impl Store {
    pub fn open(dir: &Path, role: EndpointRole) -> Result<Self> {
        private_directory(dir).map_err(|_| Diagnostic::err(IO_FAILURE))?;
        let file = match role {
            EndpointRole::Host => "host.chain",
            EndpointRole::Client => "client.chain",
        };
        let path = dir.join(format!("{file}.lock"));
        match fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_file() || crate::config::loose_permissions(&path) => {
                return Err(Diagnostic::err(DAMAGED));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(Diagnostic::err(IO_FAILURE));
            }
            _ => {}
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(path)
            .map_err(|_| Diagnostic::err(IO_FAILURE))?;
        lock.try_lock().map_err(|_| {
            Diagnostic::err("another k-lani-link process already uses this pairing state")
        })?;
        Ok(Self {
            dir: dir.into(),
            file,
            role: role.into(),
            poisoned: false,
            _lock: lock,
        })
    }
    fn usable(&self) -> Result<()> {
        if self.poisoned {
            Err(Diagnostic::err(IO_FAILURE))
        } else {
            Ok(())
        }
    }
    fn read_raw(&self) -> Result<Option<String>> {
        self.usable()?;
        let path = self.dir.join(self.file);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Diagnostic::err(DAMAGED)),
        };
        if !metadata.is_file()
            || metadata.len() > MAX_FILE_BYTES
            || crate::config::loose_permissions(&path)
        {
            return Err(Diagnostic::err(DAMAGED));
        }
        let mut text = String::new();
        File::open(path)
            .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_string(&mut text))
            .map_err(|_| Diagnostic::err(DAMAGED))?;
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(Diagnostic::err(DAMAGED));
        }
        Ok(Some(text))
    }
    fn read(&self) -> Result<Disk> {
        let text = self.read_raw()?.ok_or_else(|| Diagnostic::err(MISSING))?;
        let json = text
            .strip_prefix(HEADER)
            .and_then(|s| s.strip_suffix('\n'))
            .ok_or_else(|| Diagnostic::err(DAMAGED))?;
        let value: Disk = serde_json::from_str(json).map_err(|_| Diagnostic::err(DAMAGED))?;
        if value.role() != self.role {
            return Err(Diagnostic::err(DAMAGED));
        }
        if matches!(value, Disk::Revoked { .. }) {
            return Err(Diagnostic::err(REVOKED));
        }
        Ok(value)
    }
    fn write(&mut self, value: Disk) -> Result<()> {
        self.usable()?;
        if value.role() != self.role {
            return Err(Diagnostic::err(ROLE));
        }
        let text = format!(
            "{HEADER}{}\n",
            serde_json::to_string(&value).map_err(|_| Diagnostic::err(DAMAGED))?
        );
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(Diagnostic::err(DAMAGED));
        }
        let operation = || -> std::io::Result<()> {
            step(Step::BeforeWrite)?;
            let temporary = self.dir.join(format!("{}.tmp", self.file));
            match fs::remove_file(&temporary) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(text.as_bytes())?;
            step(Step::Written)?;
            file.sync_all()?;
            step(Step::Synced)?;
            drop(file);
            fs::rename(&temporary, self.dir.join(self.file))?;
            step(Step::Renamed)?;
            #[cfg(unix)]
            File::open(&self.dir)?.sync_all()?;
            step(Step::DirectorySynced)?;
            Ok(())
        };
        if operation().is_err() {
            self.poisoned = true;
            return Err(Diagnostic::err(IO_FAILURE));
        }
        Ok(())
    }
    /// Explicit issuance only. A future pairing command must complete BOTH endpoint initializations before export.
    pub fn initialize(&mut self, initial: &Chain, now: u64, limits: Lifetimes) -> Result<()> {
        if self.read_raw()?.is_some() {
            return Err(Diagnostic::err(EXISTS));
        }
        let lifecycle = Lifecycle::new(now, limits)?;
        let value = match self.role {
            Role::Host => host_disk(&HostState {
                chains: HostChains::new(initial.clone()),
                lifecycle,
            }),
            Role::Client => Disk::Client {
                lifecycle,
                cur: encode(initial.as_bytes()),
            },
        };
        self.write(value)
    }
    pub fn host(&self, now: u64) -> Result<HostState> {
        if self.role != Role::Host {
            return Err(Diagnostic::err(ROLE));
        }
        let Disk::Host {
            lifecycle,
            cur,
            pending,
            retired,
        } = self.read()?
        else {
            return Err(Diagnostic::err(DAMAGED));
        };
        lifecycle.check(now)?;
        if pending.len() > MAX_PENDING || retired.len() > MAX_RETIRED {
            return Err(Diagnostic::err(DAMAGED));
        }
        let pending = pending
            .iter()
            .map(|s| decode::<PENDING_BYTES>(s).map(|bytes| Pending::from_bytes(&bytes)))
            .collect::<Result<Vec<_>>>()?;
        let retired = retired
            .iter()
            .map(|s| {
                decode::<RETIRED_GENERATION_BYTES>(s)
                    .map(|bytes| RetiredGeneration::from_bytes(&bytes))
            })
            .collect::<Result<Vec<_>>>()?;
        let chains = HostChains::restore(Chain::from_bytes(decode(&cur)?), pending, retired)
            .map_err(|_| Diagnostic::err(DAMAGED))?;
        Ok(HostState { chains, lifecycle })
    }
    pub fn client(&self, now: u64) -> Result<ClientState> {
        if self.role != Role::Client {
            return Err(Diagnostic::err(ROLE));
        }
        let Disk::Client { lifecycle, cur } = self.read()? else {
            return Err(Diagnostic::err(DAMAGED));
        };
        lifecycle.check(now)?;
        Ok(ClientState {
            chain: Chain::from_bytes(decode(&cur)?),
            lifecycle,
        })
    }
    /// A new reply is returned only after its pending transaction is durably saved.
    pub fn accept_host(
        &mut self,
        room: &str,
        hello: &[u8],
        secret: [u8; 32],
        nonce: [u8; 24],
        now: u64,
    ) -> Result<Verdict> {
        let mut state = self.host(now)?;
        let verdict = state
            .chains
            .accept(room, hello, secret, nonce, now)
            .map_err(|e| Diagnostic::err(e.to_string()))?;
        if let Verdict::Accepted(accepted) = &verdict {
            state.chains = state
                .chains
                .after_accept(accepted)
                .map_err(|e| Diagnostic::err(e.to_string()))?;
            state
                .lifecycle
                .observe(now, accepted.via() == Via::Pending)?;
            self.write(host_disk(&state))?;
        }
        Ok(verdict)
    }
    /// Caller sends the first Ready only after this method succeeds. Host validity remains authoritative.
    pub fn commit_client(&mut self, established: &Established, now: u64) -> Result<()> {
        let mut state = self.client(now)?;
        if state.chain.room() != established.source_room() {
            return Err(Diagnostic::err(
                "the handshake no longer matches the client state",
            ));
        }
        state.lifecycle.observe(now, true)?;
        self.write(Disk::Client {
            lifecycle: state.lifecycle,
            cur: encode(established.next().as_bytes()),
        })
    }
    /// Initial Ready must verify against a still-pending live attempt. Host Ready/data follow durable success only.
    pub fn confirm_host(&mut self, accepted: &mut Accepted, ready: &[u8], now: u64) -> Result<()> {
        let mut state = self.host(now)?;
        state.chains = state
            .chains
            .confirm(accepted, ready)
            .map_err(|e| Diagnostic::err(e.to_string()))?;
        state.lifecycle.observe(now, true)?;
        self.write(host_disk(&state))
    }
    /// Call only for independently verified activity in a paired, live session. Never for HTTP responses or replays.
    pub fn record_verified_activity(&mut self, now: u64) -> Result<()> {
        match self.role {
            Role::Host => {
                let mut state = self.host(now)?;
                if !state.lifecycle.paired {
                    return Err(Diagnostic::err(
                        "unpaired activity cannot renew the issuance lifetime",
                    ));
                }
                state.lifecycle.observe(now, true)?;
                self.write(host_disk(&state))
            }
            Role::Client => {
                let mut state = self.client(now)?;
                if !state.lifecycle.paired {
                    return Err(Diagnostic::err(
                        "unpaired activity cannot renew the issuance lifetime",
                    ));
                }
                state.lifecycle.observe(now, true)?;
                self.write(Disk::Client {
                    lifecycle: state.lifecycle,
                    cur: encode(state.chain.as_bytes()),
                })
            }
        }
    }
    /// Stop the endpoint and discard live sessions before revocation. The store lock prevents a second compliant owner.
    pub fn revoke(&mut self) -> Result<()> {
        self.read_raw()?; // Verify file shape/permissions when present; never load or recreate a credential.
        self.write(Disk::Revoked { role: self.role })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::random_id;
    use k_lani_link_protocol::{Frame, Initiator};
    const NOW: u64 = 1_000_000;
    const STEPS: [Step; 5] = [
        Step::BeforeWrite,
        Step::Written,
        Step::Synced,
        Step::Renamed,
        Step::DirectorySynced,
    ];
    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("link-state-{}", random_id().unwrap()))
    }
    fn initial() -> Chain {
        Chain::from_bytes([1; 32])
    }
    fn init(chain: &Chain, seed: u8, now: u64) -> Initiator {
        Initiator::start(chain, [seed; 32], [seed; 16], [seed; 24], now).unwrap()
    }
    fn accept(host: &mut Store, hello: &Initiator, seed: u8, now: u64) -> Accepted {
        let Verdict::Accepted(value) = host
            .accept_host(&hello.room(), hello.hello(), [seed; 32], [seed; 24], now)
            .unwrap()
        else {
            panic!("expected fresh handshake");
        };
        value
    }
    fn pair(dir: &Path, limits: Lifetimes) -> (Store, Store) {
        let mut host = Store::open(dir, EndpointRole::Host).unwrap();
        let mut client = Store::open(dir, EndpointRole::Client).unwrap();
        host.initialize(&initial(), NOW, limits).unwrap();
        client.initialize(&initial(), NOW, limits).unwrap();
        (host, client)
    }
    fn connect(host: &mut Store, client: &mut Store, seed: u8, now: u64) {
        let hello = init(&client.client(now).unwrap().chain, seed, now);
        let mut accepted = accept(host, &hello, seed + 1, now);
        let mut established = hello.finish(accepted.reply(), now).unwrap();
        client.commit_client(&established, now).unwrap();
        let ready = established.session.seal_batch(&[Frame::Ready]).unwrap();
        host.confirm_host(&mut accepted, &ready, now).unwrap();
        let host_ready = accepted.session.seal_batch(&[Frame::Ready]).unwrap();
        assert!(matches!(
            established
                .session
                .open_batch(&host_ready)
                .unwrap()
                .as_slice(),
            [Frame::Ready]
        ));
        assert!(
            host.host(now).unwrap().chains.current().as_bytes()
                == client.client(now).unwrap().chain.as_bytes()
        );
    }
    #[test]
    fn issuance_is_explicit_and_expiry_does_not_restart_on_load() {
        let dir = scratch();
        let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
        assert_eq!(host.host(NOW).err().unwrap().to_string(), MISSING);
        assert!(!dir.join("host.chain").exists());
        host.initialize(&initial(), NOW, Lifetimes::default())
            .unwrap();
        assert!(
            host.initialize(&initial(), NOW + 1, Lifetimes::default())
                .is_err()
        );
        assert_eq!(
            host.host(NOW + 1).unwrap().lifecycle.expires_at(),
            NOW + 86_400
        );
        assert_eq!(host.host(NOW + 86_400).err().unwrap().to_string(), EXPIRED);
        assert!(host.record_verified_activity(NOW + 2).is_err());
        assert_eq!(host.host(NOW - 1).err().unwrap().to_string(), CLOCK);
        drop(host);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn paired_idle_refresh_never_extends_an_absolute_deadline() {
        let dir = scratch();
        let limits = Lifetimes {
            unused_seconds: 10,
            idle_seconds: 20,
            absolute_seconds: Some(30),
        };
        let (mut host, mut client) = pair(&dir, limits);
        connect(&mut host, &mut client, 2, NOW + 1);
        assert_eq!(host.host(NOW + 1).unwrap().lifecycle.expires_at(), NOW + 21);
        host.record_verified_activity(NOW + 19).unwrap();
        client.record_verified_activity(NOW + 19).unwrap();
        assert_eq!(
            host.host(NOW + 19).unwrap().lifecycle.expires_at(),
            NOW + 30
        );
        assert_eq!(
            client.client(NOW + 19).unwrap().lifecycle.expires_at(),
            NOW + 30
        );
        assert_eq!(host.host(NOW + 18).err().unwrap().to_string(), CLOCK);
        assert_eq!(host.host(NOW + 30).err().unwrap().to_string(), EXPIRED);
        assert_eq!(client.client(NOW + 30).err().unwrap().to_string(), EXPIRED);
        assert!(host.record_verified_activity(NOW + 30).is_err());
        drop(host);
        drop(client);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn invalid_or_overflowing_lifetime_policies_are_not_written() {
        for limits in [
            Lifetimes {
                unused_seconds: 0,
                ..Lifetimes::default()
            },
            Lifetimes {
                idle_seconds: u64::MAX,
                ..Lifetimes::default()
            },
            Lifetimes {
                absolute_seconds: Some(0),
                ..Lifetimes::default()
            },
            Lifetimes {
                absolute_seconds: Some(u64::MAX),
                ..Lifetimes::default()
            },
        ] {
            let dir = scratch();
            let mut store = Store::open(&dir, EndpointRole::Host).unwrap();
            assert!(store.initialize(&initial(), NOW, limits).is_err());
            assert!(!dir.join("host.chain").exists());
            drop(store);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn four_pending_and_three_retired_generations_round_trip_without_renewing_on_replay() {
        let dir = scratch();
        let (mut host, mut client) = pair(&dir, Lifetimes::default());
        for seed in [2, 6, 10] {
            connect(&mut host, &mut client, seed, NOW + 1);
        }
        let current = client.client(NOW + 2).unwrap().chain;
        let before = host.host(NOW + 2).unwrap().lifecycle.expires_at();
        let mut hellos = Vec::new();
        for seed in [20, 22, 24, 26] {
            let hello = init(&current, seed, NOW + 2);
            accept(&mut host, &hello, seed + 1, NOW + 2);
            hellos.push(hello);
        }
        drop(host);
        let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
        let state = host.host(NOW + 3).unwrap();
        assert_eq!(state.chains.pending().len(), 4);
        assert_eq!(state.chains.retired().len(), 3);
        let expected = state.chains.pending()[0].reply().to_vec();
        let replay = host
            .accept_host(
                &hellos[0].room(),
                hellos[0].hello(),
                [80; 32],
                [80; 24],
                NOW + 500,
            )
            .unwrap();
        let Verdict::CachedReply(reply) = replay else {
            panic!("expected saved reply only");
        };
        assert_eq!(reply, expected);
        assert_eq!(host.host(NOW + 500).unwrap().lifecycle.expires_at(), before);
        drop(host);
        drop(client);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn revoked_state_contains_no_keys_cannot_be_overwritten_and_requires_process_stop() {
        let dir = scratch();
        let (mut host, mut client) = pair(&dir, Lifetimes::default());
        connect(&mut host, &mut client, 2, NOW + 1);
        let secret = encode(host.host(NOW + 1).unwrap().chains.current().as_bytes());
        assert!(Store::open(&dir, EndpointRole::Host).is_err());
        host.revoke().unwrap();
        client.revoke().unwrap();
        let text = fs::read_to_string(dir.join("host.chain")).unwrap();
        assert!(!text.contains(&secret));
        assert_eq!(host.host(NOW + 2).err().unwrap().to_string(), REVOKED);
        assert!(
            host.initialize(&initial(), NOW + 2, Lifetimes::default())
                .is_err()
        );
        assert!(host.record_verified_activity(NOW + 2).is_err());
        assert_eq!(client.client(NOW + 2).err().unwrap().to_string(), REVOKED);
        drop(host);
        drop(client);
        let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
        assert_eq!(host.host(NOW + 2).err().unwrap().to_string(), REVOKED);
        fs::remove_file(dir.join("host.chain")).unwrap();
        assert_eq!(host.host(NOW + 2).err().unwrap().to_string(), MISSING);
        host.revoke().unwrap();
        assert_eq!(host.host(NOW + 2).err().unwrap().to_string(), REVOKED);
        drop(host);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn corrupt_unsupported_or_over_limit_state_is_left_untouched_with_fixed_diagnostics() {
        let dir = scratch();
        let (host, client) = pair(&dir, Lifetimes::default());
        let path = dir.join("host.chain");
        let good = fs::read_to_string(&path).unwrap();
        let secret = encode(initial().as_bytes());
        let mut json: serde_json::Value =
            serde_json::from_str(good.strip_prefix(HEADER).unwrap()).unwrap();
        json["pending"] = serde_json::json!(["x", "x", "x", "x", "x"]);
        for bad in [
            String::new(),
            good.replace("chain v2", "chain v1"),
            good.replace("\"host\"", "\"client\""),
            good.replace(&secret, "not-a-key"),
            good.trim_end().into(),
            format!("{good}garbage"),
            format!("{HEADER}{json}\n"),
            "x".repeat(MAX_FILE_BYTES as usize + 1),
        ] {
            fs::write(&path, &bad).unwrap();
            let error = host.host(NOW).err().unwrap();
            assert_eq!(error.to_string(), DAMAGED);
            assert_eq!(
                crate::diagnostic::printable(&error).as_deref(),
                Some(DAMAGED)
            );
            assert!(!error.to_string().contains(&secret));
            assert_eq!(fs::read_to_string(&path).unwrap(), bad);
        }
        drop(host);
        drop(client);
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn state_and_lock_must_be_private_regular_files() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let dir = scratch();
        let (host, client) = pair(&dir, Lifetimes::default());
        let path = dir.join("host.chain");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(host.host(NOW).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let elsewhere = dir.join("elsewhere");
        fs::rename(&path, &elsewhere).unwrap();
        symlink(&elsewhere, &path).unwrap();
        assert!(host.host(NOW).is_err());
        drop(host);
        drop(client);
        let lock = dir.join("host.chain.lock");
        fs::remove_file(&lock).unwrap();
        symlink(&elsewhere, &lock).unwrap();
        assert!(Store::open(&dir, EndpointRole::Host).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn failed_save_never_exposes_a_new_reply_and_poisons_the_store_until_reopen() {
        for stage in STEPS {
            let dir = scratch();
            let (mut host, client) = pair(&dir, Lifetimes::default());
            let hello = init(&initial(), 2, NOW);
            CRASH.with(|c| c.set(Some(stage)));
            let error = host
                .accept_host(&hello.room(), hello.hello(), [3; 32], [3; 24], NOW)
                .err()
                .unwrap();
            CRASH.with(|c| c.set(None));
            assert_eq!(error.to_string(), IO_FAILURE);
            assert_eq!(host.host(NOW).err().unwrap().to_string(), IO_FAILURE);
            assert!(host.revoke().is_err());
            drop(host);
            drop(client);
            let host = Store::open(&dir, EndpointRole::Host).unwrap();
            let count = host.host(NOW).unwrap().chains.pending().len();
            assert_eq!(
                count,
                usize::from(matches!(stage, Step::Renamed | Step::DirectorySynced))
            );
            drop(host);
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn dropping_store_releases_lock_even_while_a_duplicate_handle_exists() {
        let dir = scratch();
        let host = Store::open(&dir, EndpointRole::Host).unwrap();
        // A concurrent fork can retain the same open file description until exec.
        // Keep a duplicate alive to reproduce that window without timing or unsafe code.
        let inherited = host._lock.try_clone().unwrap();
        assert!(Store::open(&dir, EndpointRole::Host).is_err());
        drop(host);
        let reopened = Store::open(&dir, EndpointRole::Host).unwrap();
        drop(inherited);
        assert!(Store::open(&dir, EndpointRole::Host).is_err());
        drop(reopened);
        fs::remove_dir_all(dir).unwrap();
    }

    /// Invoked as a separate test process by the parent below, never in an ordinary unit run.
    #[test]
    #[ignore = "child process of the crash matrix"]
    fn crash_child() {
        let Ok(dir) = std::env::var("LINK_STATE_CRASH_DIR") else {
            return;
        };
        let operation = std::env::var("LINK_STATE_CRASH_OPERATION").unwrap();
        let stage = std::env::var("LINK_STATE_ARM_STEP").unwrap();
        // Environment mutation is avoided: select a thread-local injected exit stage instead.
        let at = match stage.as_str() {
            "BeforeWrite" => Step::BeforeWrite,
            "Written" => Step::Written,
            "Synced" => Step::Synced,
            "Renamed" => Step::Renamed,
            "DirectorySynced" => Step::DirectorySynced,
            _ => panic!("invalid test stage"),
        };
        let mut host = Store::open(Path::new(&dir), EndpointRole::Host).unwrap();
        let mut client = Store::open(Path::new(&dir), EndpointRole::Client).unwrap();
        if operation == "init-host" || operation == "init-client" {
            EXIT_AT.with(|s| s.set(Some(at)));
            if operation == "init-host" {
                host.initialize(&initial(), NOW, Lifetimes::default())
                    .unwrap();
            } else {
                client
                    .initialize(&initial(), NOW, Lifetimes::default())
                    .unwrap();
            }
            panic!("fault not reached");
        }
        if operation == "revoke-host" || operation == "revoke-client" {
            EXIT_AT.with(|s| s.set(Some(at)));
            if operation == "revoke-host" {
                host.revoke().unwrap();
            } else {
                client.revoke().unwrap();
            }
            panic!("fault not reached");
        }
        let hello = init(&initial(), 2, NOW);
        if operation == "accept" {
            EXIT_AT.with(|s| s.set(Some(at)));
        }
        let mut accepted = accept(&mut host, &hello, 3, NOW);
        let mut established = hello.finish(accepted.reply(), NOW).unwrap();
        if operation == "client" {
            EXIT_AT.with(|s| s.set(Some(at)));
        }
        client.commit_client(&established, NOW).unwrap();
        let ready = established.session.seal_batch(&[Frame::Ready]).unwrap();
        if operation == "confirm" {
            EXIT_AT.with(|s| s.set(Some(at)));
        }
        host.confirm_host(&mut accepted, &ready, NOW).unwrap();
        EXIT_AT.with(|s| s.set(Some(at)));
        if operation == "activity-host" {
            host.record_verified_activity(NOW + 10).unwrap();
        } else if operation == "activity-client" {
            client.record_verified_activity(NOW + 10).unwrap();
        }
        panic!("fault not reached");
    }
    #[test]
    fn actual_process_exit_at_every_persistence_boundary_reopens_without_losing_the_client() {
        for operation in [
            "init-host",
            "init-client",
            "accept",
            "client",
            "confirm",
            "activity-host",
            "activity-client",
            "revoke-host",
            "revoke-client",
        ] {
            for stage in STEPS {
                let dir = scratch();
                if !operation.starts_with("init-") {
                    let (h, c) = pair(&dir, Lifetimes::default());
                    drop(h);
                    drop(c);
                }
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "chain_state::tests::crash_child",
                        "--ignored",
                        "--test-threads=1",
                    ])
                    .env("LINK_STATE_CRASH_DIR", &dir)
                    .env("LINK_STATE_CRASH_OPERATION", operation)
                    .env("LINK_STATE_ARM_STEP", format!("{stage:?}"))
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(91),
                    "child did not exit at the selected boundary: {operation} {stage:?}"
                );
                let committed = matches!(stage, Step::Renamed | Step::DirectorySynced);
                let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
                let mut client = Store::open(&dir, EndpointRole::Client).unwrap();
                match operation {
                    "init-host" => {
                        assert_eq!(host.host(NOW).is_ok(), committed);
                        if !committed {
                            assert_eq!(host.host(NOW).err().unwrap().to_string(), MISSING);
                        }
                    }
                    "init-client" => {
                        assert_eq!(client.client(NOW).is_ok(), committed);
                    }
                    "revoke-host" => {
                        assert_eq!(host.host(NOW).is_ok(), !committed);
                        host.revoke().unwrap();
                        assert_eq!(host.host(NOW).err().unwrap().to_string(), REVOKED);
                    }
                    "revoke-client" => {
                        assert_eq!(client.client(NOW).is_ok(), !committed);
                        client.revoke().unwrap();
                        assert_eq!(client.client(NOW).err().unwrap().to_string(), REVOKED);
                    }
                    "accept" => {
                        assert_eq!(
                            host.host(NOW).unwrap().chains.pending().len(),
                            usize::from(committed)
                        );
                        connect(&mut host, &mut client, 10, NOW + 20);
                    }
                    "client" | "confirm" => {
                        connect(&mut host, &mut client, 10, NOW + 20);
                    }
                    "activity-host" => {
                        assert_eq!(
                            host.host(NOW + 20).unwrap().lifecycle.last_activity,
                            if committed { NOW + 10 } else { NOW }
                        );
                        connect(&mut host, &mut client, 10, NOW + 20);
                    }
                    "activity-client" => {
                        assert_eq!(
                            client.client(NOW + 20).unwrap().lifecycle.last_activity,
                            if committed { NOW + 10 } else { NOW }
                        );
                        connect(&mut host, &mut client, 10, NOW + 20);
                    }
                    _ => unreachable!(),
                }
                drop(host);
                drop(client);
                fs::remove_dir_all(dir).unwrap();
            }
        }
    }
}
