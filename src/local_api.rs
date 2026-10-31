//! Private Unix listener for a network-isolated API consumer.
#[cfg(unix)]
pub struct Socket {
    path: std::path::PathBuf,
    identity: (u64, u64),
}

#[cfg(unix)]
impl Socket {
    pub fn bind(path: &std::path::Path) -> anyhow::Result<(Self, tokio::net::UnixListener)> {
        use anyhow::ensure;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(path.is_absolute(), "API socket path must be absolute");
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing socket directory"))?;
        let metadata = std::fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.permissions().mode() & 0o077 == 0,
            "API socket directory must be private"
        );
        // bind refuses to overwrite an existing file or socket, including symlinks.
        let listener = tokio::net::UnixListener::bind(path)?;
        let bound = std::fs::symlink_metadata(path)?;
        let socket = Self {
            path: path.into(),
            identity: (bound.dev(), bound.ino()),
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        ensure!(
            std::fs::metadata(path)?.uid() == metadata.uid(),
            "API socket owner mismatch"
        );
        Ok((socket, listener))
    }
}
#[cfg(unix)]
impl Drop for Socket {
    fn drop(&mut self) {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && (metadata.dev(), metadata.ino()) == self.identity
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
