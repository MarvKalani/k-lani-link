//! Operator-only model control through the existing encrypted peer connection.
use crate::{
    config::{Config, token_matches},
    diagnostic::Diagnostic,
};
use anyhow::Result;
use std::{io::Read, path::Path};

/// Every reason is a fixed text: a wrong key file must be diagnosable without echoing it.
pub fn token(config: &Config) -> Result<Option<String>> {
    let Some(path) = &config.management_token_file else {
        return Ok(None);
    };
    let io_reason = |error: &std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => "management token file not found",
        std::io::ErrorKind::PermissionDenied => {
            "management token file is not readable by this user"
        }
        _ => "management token file cannot be read",
    };
    let metadata = std::fs::symlink_metadata(path).map_err(|e| Diagnostic::err(io_reason(&e)))?;
    if !metadata.is_file() {
        return Err(Diagnostic::err(
            "management token path must be a regular file, not a link or directory",
        ));
    }
    if metadata.len() > 4096 {
        return Err(Diagnostic::err(
            "management token file is larger than 4 KiB",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Diagnostic::err(
                "management token file must not be accessible by group or others (chmod 600)",
            ));
        }
    }
    let mut value = String::new();
    std::fs::File::open(path)
        .map_err(|e| Diagnostic::err(io_reason(&e)))?
        .take(4097)
        .read_to_string(&mut value)
        .map_err(|_| Diagnostic::err("management token must be plain ASCII text"))?;
    let value = value.trim().to_owned();
    if !(32..=256).contains(&value.len()) || !value.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Diagnostic::err(
            "management token must be 32 to 256 printable ASCII characters",
        ));
    }
    if token_matches(Some(&format!("Bearer {value}")), &config.api_key) {
        return Err(Diagnostic::err(
            "management token must differ from the API key",
        ));
    }
    Ok(Some(value))
}

pub async fn request(
    config: &Config,
    api_socket: Option<&Path>,
    command: Option<k_lani_link_protocol::ModelCommand>,
) -> Result<serde_json::Value> {
    let key = token(config)?
        .ok_or_else(|| Diagnostic::err("model management is not enabled on this client"))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(
            config.request_timeout_seconds,
        ));
    #[cfg(unix)]
    let client = if let Some(socket) = api_socket {
        client.unix_socket(socket)
    } else {
        client
    };
    #[cfg(not(unix))]
    if api_socket.is_some() {
        return Err(Diagnostic::err(
            "Unix API sockets are unavailable on this platform",
        ));
    }
    let client = client.build()?;
    let origin = format!("http://{}", config.listen);
    let request = if let Some(command) = command {
        let body = serde_json::to_vec(&command)?;
        if !k_lani_link_protocol::control_body_valid("POST", &body) {
            return Err(Diagnostic::err("invalid model ID"));
        }
        client
            .post(format!("{origin}/v1/link/model"))
            .header("content-type", "application/json")
            .body(body)
    } else {
        client.get(format!("{origin}/v1/link/models"))
    };
    let mut response = request
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| Diagnostic::err("model control request failed or timed out"))?;
    if !response.status().is_success() {
        return Err(Diagnostic::err(format!(
            "model control returned HTTP {}",
            response.status().as_u16()
        )));
    }
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if data.len() + chunk.len() > 64 * 1024 {
            return Err(Diagnostic::err("model control response too large"));
        }
        data.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&data).map_err(|_| Diagnostic::err("invalid model control response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reason(cfg: &Config) -> String {
        token(cfg).err().unwrap().to_string()
    }
    #[test]
    fn management_requires_a_private_independent_regular_key_file() {
        let root = std::env::temp_dir().join(format!(
            "link-control-key-{}",
            crate::config::random_id().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut cfg = crate::config::tests::config();
        assert!(
            token(&cfg).unwrap().is_none(),
            "management is off unless configured"
        );
        let path = root.join("token");
        cfg.management_token_file = Some(path.clone());
        assert_eq!(reason(&cfg), "management token file not found");
        // The inference key must not double as the operator key.
        crate::config::private_file(&path, &cfg.api_key).unwrap();
        assert_eq!(
            reason(&cfg),
            "management token must differ from the API key"
        );
        std::fs::write(&path, "short").unwrap();
        assert_eq!(
            reason(&cfg),
            "management token must be 32 to 256 printable ASCII characters"
        );
        std::fs::write(&path, "m".repeat(43)).unwrap();
        assert_eq!(token(&cfg).unwrap().unwrap().len(), 43);
        std::fs::write(&path, "m".repeat(5000)).unwrap();
        assert_eq!(reason(&cfg), "management token file is larger than 4 KiB");
        std::fs::write(&path, "m".repeat(43)).unwrap();
        std::fs::create_dir(root.join("dir")).unwrap();
        cfg.management_token_file = Some(root.join("dir"));
        assert_eq!(
            reason(&cfg),
            "management token path must be a regular file, not a link or directory"
        );
        cfg.management_token_file = Some(path.clone());
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                reason(&cfg),
                "management token file must not be accessible by group or others (chmod 600)"
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let link = root.join("link");
            symlink(&path, &link).unwrap();
            cfg.management_token_file = Some(link);
            assert_eq!(
                reason(&cfg),
                "management token path must be a regular file, not a link or directory"
            );
        }
        // No reason ever repeats the key material.
        cfg.management_token_file = Some(path.clone());
        std::fs::write(&path, format!("{}\n{}", "m".repeat(20), "m".repeat(20))).unwrap();
        assert!(!reason(&cfg).contains(&"m".repeat(8)));
        std::fs::remove_dir_all(root).unwrap();
    }
}
