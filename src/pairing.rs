//! A rejected peer packet is terminal for this pairing attempt, never a retry hint.
use crate::{config::Config, signal::now};
use anyhow::{Context, Result, ensure};
use k_lani_link_protocol::{EndpointRole, Signal};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct PairingRejected;
impl std::fmt::Display for PairingRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "pairing rejected: the peer could not be authenticated or the rendezvous answered unexpectedly (check the connection code, the signaling URL and any proxy, and that both clocks are within 30 s)",
        )
    }
}
impl std::error::Error for PairingRejected {}

#[derive(Debug)]
pub struct CodeUnavailable;
impl std::fmt::Display for CodeUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "connection code is used or already reserved; if an earlier run was killed before pairing finished, create a new code",
        )
    }
}
impl std::error::Error for CodeUnavailable {}

/// Creates the state directory (or accepts an existing real one) and insists on mode 0700.
pub fn private_directory(directory: &Path) -> Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)?;
    let metadata = fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "pairing state must be a real directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "pairing state directory must be private"
        );
    }
    Ok(())
}

/// Empty markers are authentication state, not a connection or message log.
pub struct PairingLease {
    reserved: PathBuf,
    used: PathBuf,
    #[cfg(unix)]
    directory: PathBuf,
    committed: bool,
}

impl PairingLease {
    pub fn reserve(directory: &Path, code: &str) -> Result<Self> {
        let key = k_lani_link_protocol::pair_key(code).map_err(anyhow::Error::msg)?;
        private_directory(directory)?;
        let digest =
            Sha256::digest([b"k-lani-link/one-time/v2".as_slice(), key.as_slice()].concat());
        let id: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let used = directory.join(format!("{id}.used"));
        let reserved = directory.join(format!("{id}.reserved"));
        if used.exists() {
            return Err(CodeUnavailable.into());
        }
        create_marker(&reserved).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::Error::from(CodeUnavailable)
            } else {
                anyhow::Error::from(error)
            }
        })?;
        let lease = Self {
            reserved,
            used,
            #[cfg(unix)]
            directory: directory.into(),
            committed: false,
        };
        if lease.used.exists() {
            return Err(CodeUnavailable.into());
        }
        lease.sync_directory()?;
        Ok(lease)
    }

    pub fn consume(&mut self) -> Result<()> {
        ensure!(!self.committed, "code already consumed");
        create_marker(&self.used).context("persist consumed connection code")?;
        self.committed = true;
        self.sync_directory()?;
        fs::remove_file(&self.reserved)?;
        self.sync_directory()?;
        Ok(())
    }

    fn sync_directory(&self) -> Result<()> {
        #[cfg(unix)]
        fs::File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}
impl Drop for PairingLease {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.reserved);
        }
    }
}
fn create_marker(path: &Path) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.sync_all()
}

pub fn state_directory(config: &Config, config_file: &Path) -> PathBuf {
    config.pairing_state_dir.clone().unwrap_or_else(|| {
        config_file
            .parent()
            .unwrap_or(Path::new("."))
            .join(".link-key-state")
    })
}

pub fn open(
    signal: &Signal,
    config: &Config,
    kind: &str,
    role: EndpointRole,
) -> anyhow::Result<String> {
    signal
        .open(&config.pair_key, &config.room, kind, now(), role)
        .map_err(|_| PairingRejected.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_is_exclusive_and_consumption_survives_a_new_instance() {
        let root =
            std::env::temp_dir().join(format!("link-once-{}", crate::config::random_id().unwrap()));
        let code = crate::config::random_secret().unwrap();
        let first = PairingLease::reserve(&root, &code).unwrap();
        assert!(PairingLease::reserve(&root, &code).is_err());
        drop(first); // Failure before the authenticated tunnel does not consume the code.
        let mut second = PairingLease::reserve(&root, &code).unwrap();
        second.consume().unwrap();
        drop(second);
        assert!(
            PairingLease::reserve(&root, &code)
                .err()
                .unwrap()
                .is::<CodeUnavailable>()
        );
        let entries: Vec<_> = fs::read_dir(&root).unwrap().collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].as_ref().unwrap().metadata().unwrap().len(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
