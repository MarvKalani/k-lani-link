//! Signed self-updates from the project's GitHub releases. A release counts only when its
//! SHA256SUMS carries a valid Ed25519 signature from the key below, the executable for
//! this platform matches it, and its version is newer than the running one, so an older
//! signed release cannot be replayed. Only the control panel checks by itself; hosts and
//! clients update when an operator runs `k-lani-link update`.
use crate::diagnostic::Diagnostic;
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The release signing key (`deploy/portable/sign.py`); its private half never leaves the
/// maintainer's computer.
const PUBLIC_KEY: &str = "mRnYdQbBBf/BiEEms/G3kZ6q25bp7yAPVsbrB9qT1HA=";
const RELEASES: &str = "https://api.github.com/repos/MarvKalani/k-lani-link/releases?per_page=20";
/// A distribution built with `K_LANI_LINK_NO_UPDATES` set never looks for updates.
pub const BUILT_IN: bool = option_env!("K_LANI_LINK_NO_UPDATES").is_none();
/// The release asset name of this build's platform, as `deploy/portable/stage.py` names it.
pub const PLATFORM: Option<&str> = if cfg!(all(windows, target_arch = "x86_64")) {
    Some("windows-x64")
} else if cfg!(all(windows, target_arch = "aarch64")) {
    Some("windows-arm64")
} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
    Some("macos-x64")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("macos-arm64")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("linux-x64")
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    Some("linux-arm64")
} else {
    None
};
const MAX_BINARY: usize = 96 * 1024 * 1024;
const MAX_SUMS: usize = 64 * 1024;
const SUMS: &str = "SHA256SUMS";
const SIGNATURE: &str = "SHA256SUMS.sig";

pub fn running() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver")
}
fn asset(version: &Version, platform: &str) -> String {
    let suffix = if platform.starts_with("windows-") {
        ".exe"
    } else {
        ""
    };
    format!("k-lani-link-{version}-{platform}{suffix}")
}

/// Asks GitHub only for public release data; the user agent names the running version.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("k-lani-link/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            let github = url.host_str().is_some_and(|host| {
                host == "github.com"
                    || host.ends_with(".github.com")
                    || host.ends_with(".githubusercontent.com")
            });
            if attempt.previous().len() < 5 && url.scheme() == "https" && github {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(300))
        .build()
}

#[derive(Deserialize)]
struct Listed {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}
/// A newer release with everything an update needs.
pub struct Release {
    pub version: Version,
    name: String,
    binary: String,
    sums: String,
    signature: String,
}

fn choose(listed: Vec<Listed>, running: &Version, platform: &str) -> Option<Release> {
    listed
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
            if version <= *running {
                return None;
            }
            let name = asset(&version, platform);
            let url = |wanted: &str| {
                release
                    .assets
                    .iter()
                    .find(|asset| asset.name == wanted)
                    .map(|asset| asset.browser_download_url.clone())
            };
            Some(Release {
                binary: url(&name)?,
                sums: url(SUMS)?,
                signature: url(SIGNATURE)?,
                name,
                version,
            })
        })
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// The newest signed-looking release newer than this one, if any. `None` also for builds
/// without updates or without a published platform.
pub async fn latest(http: &reqwest::Client) -> Result<Option<Release>> {
    let Some(platform) = PLATFORM.filter(|_| BUILT_IN) else {
        return Ok(None);
    };
    latest_from(http, RELEASES, platform, &running()).await
}
async fn latest_from(
    http: &reqwest::Client,
    url: &str,
    platform: &str,
    running: &Version,
) -> Result<Option<Release>> {
    let listed: Vec<Listed> = serde_json::from_slice(&fetch(http, url, 2 * 1024 * 1024).await?)
        .map_err(|_| Diagnostic::err("GitHub returned an unexpected release list"))?;
    Ok(choose(listed, running, platform))
}

async fn fetch(http: &reqwest::Client, url: &str, cap: usize) -> Result<Vec<u8>> {
    let mut response = http
        .get(url)
        .send()
        .await
        .map_err(|_| Diagnostic::err("GitHub cannot be reached"))?;
    ensure!(
        response.status().is_success(),
        Diagnostic::err("GitHub refused the update request; try again later")
    );
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Diagnostic::err("the update download was interrupted"))?
    {
        ensure!(
            body.len() + chunk.len() <= cap,
            Diagnostic::err("the update download is larger than expected")
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks the signature over the checksum list, then the executable against its line.
pub(crate) fn verify(
    key: &[u8],
    sums: &[u8],
    signature: &[u8],
    name: &str,
    binary: &[u8],
) -> Result<()> {
    let invalid = || Diagnostic::err("the update is not signed by the k-lani-link release key");
    let signature = STANDARD
        .decode(signature.trim_ascii())
        .map_err(|_| invalid())?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(sums, &signature)
        .map_err(|_| invalid())?;
    let listed = std::str::from_utf8(sums)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (hash, file) = line.split_once("  ")?;
                (file == name).then_some(hash.to_owned())
            })
        })
        .ok_or_else(|| Diagnostic::err("the signed checksum list does not name this update"))?;
    ensure!(
        listed == hex(&Sha256::digest(binary)),
        Diagnostic::err("the downloaded update does not match its signed checksum")
    );
    Ok(())
}
fn key() -> Vec<u8> {
    STANDARD
        .decode(PUBLIC_KEY)
        .expect("the release key is base64")
}

/// Downloads and verifies a release into the private folder `stage`, replacing anything
/// staged before.
pub async fn download(http: &reqwest::Client, release: &Release, stage: &Path) -> Result<()> {
    download_with(http, release, stage, &key()).await
}
async fn download_with(
    http: &reqwest::Client,
    release: &Release,
    stage: &Path,
    key: &[u8],
) -> Result<()> {
    let sums = fetch(http, &release.sums, MAX_SUMS).await?;
    let signature = fetch(http, &release.signature, 1024).await?;
    let binary = fetch(http, &release.binary, MAX_BINARY).await?;
    verify(key, &sums, &signature, &release.name, &binary)?;
    write_stage(stage, &sums, &signature, &release.name, &binary)
}
fn write_stage(
    stage: &Path,
    sums: &[u8],
    signature: &[u8],
    name: &str,
    binary: &[u8],
) -> Result<()> {
    let _ = std::fs::remove_dir_all(stage);
    crate::pairing::private_directory(stage)?;
    for (file, data) in [(SUMS, sums), (SIGNATURE, signature), (name, binary)] {
        let partial = stage.join(format!("{file}.part"));
        std::fs::write(&partial, data)?;
        std::fs::rename(&partial, stage.join(file))?;
    }
    Ok(())
}

/// A verified executable waiting in `stage`.
pub struct Staged {
    pub version: Version,
    path: PathBuf,
}
/// The update in `stage` if it is complete, still verifies and is newer than this build.
pub fn staged(stage: &Path) -> Option<Staged> {
    staged_with(stage, &key(), PLATFORM?, &running())
}
fn staged_with(stage: &Path, key: &[u8], platform: &str, running: &Version) -> Option<Staged> {
    let sums = std::fs::read(stage.join(SUMS)).ok()?;
    let signature = std::fs::read(stage.join(SIGNATURE)).ok()?;
    let entries = std::fs::read_dir(stage).ok()?;
    entries.flatten().find_map(|entry| {
        let name = entry.file_name().into_string().ok()?;
        let version = name
            .strip_prefix("k-lani-link-")?
            .strip_suffix(".exe")
            .unwrap_or(&name["k-lani-link-".len()..])
            .strip_suffix(&format!("-{platform}"))
            .and_then(|version| Version::parse(version).ok())?;
        if version <= *running || name != asset(&version, platform) {
            return None;
        }
        let binary = std::fs::read(entry.path()).ok()?;
        verify(key, &sums, &signature, &name, &binary).ok()?;
        Some(Staged {
            version,
            path: entry.path(),
        })
    })
}

/// Whether this executable's folder takes a new file, which a swap needs.
pub fn writable(exe: &Path) -> bool {
    let probe = exe.with_file_name(".k-lani-link-update-probe");
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    let _ = std::fs::remove_file(&probe);
    created
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// Replaces `exe` with the staged executable after it reported the expected version. On
/// Windows the running file moves aside to `.old`, removed at the next start.
pub fn install(staged: &Staged, exe: &Path) -> Result<()> {
    let new = sibling(exe, ".new");
    let _ = std::fs::remove_file(&new);
    std::fs::copy(&staged.path, &new)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    }
    let reported = std::process::Command::new(&new)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
    if reported.as_deref() != Some(&format!("k-lani-link {}", staged.version)) {
        let _ = std::fs::remove_file(&new);
        return Err(Diagnostic::err(
            "the new version does not start on this computer",
        ));
    }
    #[cfg(windows)]
    {
        let old = sibling(exe, ".old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)?;
        if let Err(error) = std::fs::rename(&new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(error.into());
        }
    }
    #[cfg(not(windows))]
    std::fs::rename(&new, exe)?;
    Ok(())
}

/// Removes what an earlier swap left beside the executable.
pub fn tidy(exe: &Path) {
    let _ = std::fs::remove_file(sibling(exe, ".old"));
    let _ = std::fs::remove_file(sibling(exe, ".new"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::KeyPair;

    fn keys() -> (ring::signature::Ed25519KeyPair, Vec<u8>) {
        let pkcs8 =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
                .unwrap();
        let pair = ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = pair.public_key().as_ref().to_vec();
        (pair, public)
    }
    fn signed(pair: &ring::signature::Ed25519KeyPair, sums: &str) -> String {
        STANDARD.encode(pair.sign(sums.as_bytes()).as_ref())
    }

    #[test]
    fn previews_order_before_their_release_and_the_newest_complete_release_wins() {
        let v = |text: &str| Version::parse(text).unwrap();
        assert!(v("1.2611.1-preview.1") < v("1.2611.1-preview.2"));
        assert!(v("1.2611.1-preview.2") < v("1.2611.1"));
        assert!(v("1.2610.31-preview.1") < v("1.2611.1-preview.1"));
        let release = |tag: &str, draft: bool, names: &[&str]| Listed {
            tag_name: tag.into(),
            draft,
            assets: names
                .iter()
                .map(|name| Asset {
                    name: (*name).into(),
                    browser_download_url: format!("https://github.com/x/{name}"),
                })
                .collect(),
        };
        let full = |version: &str| {
            [
                format!("k-lani-link-{version}-linux-x64"),
                SUMS.into(),
                SIGNATURE.into(),
            ]
        };
        let a = full("1.2611.2-preview.1");
        let b = full("1.2611.3-preview.1");
        let c = full("1.2611.4-preview.1");
        let listed = vec![
            release(
                "v1.2611.2-preview.1",
                false,
                &a.each_ref().map(String::as_str),
            ),
            // Newest, but without a signature: skipped.
            release(
                "v1.2611.5-preview.1",
                false,
                &["k-lani-link-1.2611.5-preview.1-linux-x64", SUMS],
            ),
            release(
                "v1.2611.4-preview.1",
                true,
                &c.each_ref().map(String::as_str),
            ),
            release(
                "v1.2611.3-preview.1",
                false,
                &b.each_ref().map(String::as_str),
            ),
            release(
                "v1.2610.31-preview.1",
                false,
                &full("1.2610.31-preview.1").each_ref().map(String::as_str),
            ),
        ];
        let chosen = choose(listed, &v("1.2611.1-preview.1"), "linux-x64").unwrap();
        assert_eq!(chosen.version, v("1.2611.3-preview.1"));
        assert!(
            chosen
                .binary
                .ends_with("k-lani-link-1.2611.3-preview.1-linux-x64")
        );
        let none = choose(
            vec![release(
                "v1.2611.1-preview.1",
                false,
                &full("1.2611.1-preview.1").each_ref().map(String::as_str),
            )],
            &v("1.2611.1-preview.1"),
            "linux-x64",
        );
        assert!(none.is_none());
    }

    #[test]
    fn only_a_signed_matching_newer_executable_is_accepted() {
        let (pair, public) = keys();
        let binary = b"new executable".to_vec();
        let name = "k-lani-link-9.0.0-linux-x64";
        let sums = format!(
            "{}  {name}\n{}  other\n",
            hex(&Sha256::digest(&binary)),
            "0".repeat(64)
        );
        let signature = signed(&pair, &sums);
        verify(
            &public,
            sums.as_bytes(),
            signature.as_bytes(),
            name,
            &binary,
        )
        .unwrap();
        let (_, stranger) = keys();
        assert!(
            verify(
                &stranger,
                sums.as_bytes(),
                signature.as_bytes(),
                name,
                &binary
            )
            .is_err()
        );
        let tampered = sums.replace("other", "others");
        assert!(
            verify(
                &public,
                tampered.as_bytes(),
                signature.as_bytes(),
                name,
                &binary
            )
            .is_err()
        );
        assert!(
            verify(
                &public,
                sums.as_bytes(),
                signature.as_bytes(),
                name,
                b"changed"
            )
            .is_err()
        );
        assert!(
            verify(
                &public,
                sums.as_bytes(),
                signature.as_bytes(),
                "k-lani-link-9.0.0-macos-arm64",
                &binary
            )
            .is_err()
        );
        assert!(verify(&public, sums.as_bytes(), b"not base64!", name, &binary).is_err());

        // Staged on disk: verified again, and never an older or equal version.
        let stage = std::env::temp_dir().join(format!(
            "link-update-{}",
            crate::config::random_id().unwrap()
        ));
        write_stage(&stage, sums.as_bytes(), signature.as_bytes(), name, &binary).unwrap();
        let found = staged_with(
            &stage,
            &public,
            "linux-x64",
            &Version::parse("1.0.0").unwrap(),
        )
        .unwrap();
        assert_eq!(found.version, Version::parse("9.0.0").unwrap());
        assert!(
            staged_with(
                &stage,
                &public,
                "linux-x64",
                &Version::parse("9.0.0").unwrap()
            )
            .is_none()
        );
        assert!(
            staged_with(
                &stage,
                &public,
                "linux-arm64",
                &Version::parse("1.0.0").unwrap()
            )
            .is_none()
        );
        std::fs::write(stage.join(name), b"swapped on disk").unwrap();
        assert!(
            staged_with(
                &stage,
                &public,
                "linux-x64",
                &Version::parse("1.0.0").unwrap()
            )
            .is_none()
        );
        std::fs::remove_dir_all(stage).unwrap();
    }

    #[tokio::test]
    async fn a_release_is_found_downloaded_verified_and_staged_or_refused_whole() {
        let (pair, public) = keys();
        let name = "k-lani-link-9.0.0-linux-x64";
        let binary = b"the next version".to_vec();
        let sums = format!("{}  {name}\n", hex(&Sha256::digest(&binary)));
        let good = signed(&pair, &sums);
        let (other, _) = keys();
        let forged = signed(&other, &sums);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let list = serde_json::json!([{"tag_name": "v9.0.0", "draft": false, "assets": [
            {"name": name, "browser_download_url": format!("{base}/bin")},
            {"name": SUMS, "browser_download_url": format!("{base}/sums")},
            {"name": SIGNATURE, "browser_download_url": format!("{base}/sig")},
        ]}, {"tag_name": "v9.0.1", "draft": false, "assets": [
            {"name": "k-lani-link-9.0.1-linux-x64", "browser_download_url": format!("{base}/bin")},
            {"name": SUMS, "browser_download_url": format!("{base}/sums")},
            {"name": SIGNATURE, "browser_download_url": format!("{base}/forged")},
        ]}]);
        let app = axum::Router::new()
            .route(
                "/releases",
                axum::routing::get(move || async move { axum::Json(list) }),
            )
            .route(
                "/bin",
                axum::routing::get(move || async move { binary.clone() }),
            )
            .route(
                "/sums",
                axum::routing::get(move || async move { sums.clone() }),
            )
            .route(
                "/sig",
                axum::routing::get(move || async move { good.clone() }),
            )
            .route(
                "/forged",
                axum::routing::get(move || async move { forged.clone() }),
            );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = client().unwrap();
        let running = Version::parse("1.0.0").unwrap();
        let stage = std::env::temp_dir().join(format!(
            "link-update-{}",
            crate::config::random_id().unwrap()
        ));
        // The newest release is forged: nothing is staged.
        let newest = latest_from(&http, &format!("{base}/releases"), "linux-x64", &running)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(newest.version, Version::parse("9.0.1").unwrap());
        assert!(
            download_with(&http, &newest, &stage, &public)
                .await
                .is_err()
        );
        assert!(!stage.exists());
        // The signed one is staged and found again.
        let older = Version::parse("9.0.0").unwrap();
        let signed_release = latest_from(&http, &format!("{base}/releases"), "linux-x64", &running)
            .await
            .unwrap()
            .map(|mut release| {
                release.version = older.clone();
                release.name = name.into();
                release.signature = format!("{base}/sig");
                release
            })
            .unwrap();
        download_with(&http, &signed_release, &stage, &public)
            .await
            .unwrap();
        let found = staged_with(&stage, &public, "linux-x64", &running).unwrap();
        assert_eq!(found.version, older);
        std::fs::remove_dir_all(stage).unwrap();
        server.abort();
    }

    #[test]
    fn the_built_in_key_is_a_valid_ed25519_public_key() {
        assert_eq!(key().len(), 32);
    }
}
