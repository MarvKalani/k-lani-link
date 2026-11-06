//! What runs inside the network-less model container: the supervisor that loads catalog
//! models on request, the single-model entrypoint, and the container health probe. These
//! replace the earlier Python scripts, so the image needs no interpreter of ours.
pub mod catalog;
pub mod manager;

use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub const RUNTIME: &str = "/run/link";
pub const CATALOG: &str = "/run/secrets/models.json";
const BASALT_CONFIG: &str = "/run/secrets/basalt.json";
const BASALT_BIN: &str = "/opt/basalt/bin/basalt";
const BASALT_OPTIONS: &str = "/opt/basalt/serve/config.py";

fn flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| value == "1")
}

/// No output a model process or this one could leave behind: stdout and stderr go nowhere,
/// new files are private, and a crash writes no core dump with prompts in it.
pub fn harden() {
    unsafe {
        let sink = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if sink >= 0 {
            libc::dup2(sink, 0);
            libc::dup2(sink, 1);
            libc::dup2(sink, 2);
            if sink > 2 {
                libc::close(sink);
            }
        }
        libc::umask(0o077);
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &none);
    }
}

/// Swap would page prompts and KV cache to disk; `/proc/swaps` then has more than its header.
pub fn host_swap_disabled() -> bool {
    std::fs::read_to_string("/proc/swaps").is_ok_and(|text| text.lines().count() == 1)
}

/// A deployment that accidentally restored a network interface is refused.
fn only_loopback() -> bool {
    std::fs::read_dir("/sys/class/net")
        .is_ok_and(|entries| entries.flatten().all(|entry| entry.file_name() == "lo"))
}

fn checked() -> Result<()> {
    ensure!(only_loopback(), "the model container must have no network");
    ensure!(
        !flag("LINK_REQUIRE_NO_SWAP") || host_swap_disabled(),
        "host swap must be disabled"
    );
    Ok(())
}

/// `backend supervise`: the catalog's models, one at a time, behind the control socket.
pub async fn supervise(stop: CancellationToken) -> Result<()> {
    harden();
    checked()?;
    let catalog = catalog::from_file(Path::new(CATALOG))?;
    let runtime = PathBuf::from(RUNTIME);
    let settings = manager::Settings::production(
        runtime.clone(),
        flag("LINK_NUMERIC_METRICS"),
        flag("LINK_REQUIRE_NO_SWAP"),
    );
    let manager = manager::Manager::new(catalog, settings, Box::new(manager::Processes));
    manager::serve(manager, &runtime.join("control.sock"), stop).await
}

/// `backend start`: one Basalt model from the operator's config, in its privacy profile.
/// Replaces this process, so Docker's signals reach Basalt directly.
pub fn start(extra: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    harden();
    checked()?;
    let socket = Path::new(RUNTIME).join("basalt.sock");
    manager::remove_socket(&socket)?;
    // An updated Basalt tree without our privacy patch must not start at all.
    let options = std::fs::read_to_string(BASALT_OPTIONS).unwrap_or_default();
    let metrics = flag("LINK_NUMERIC_METRICS");
    ensure!(
        options.contains("\"--privacy\"")
            && (!metrics || options.contains("\"--privacy-metrics-file\"")),
        "this Basalt has no privacy profile"
    );
    let mut command = std::process::Command::new(BASALT_BIN);
    command
        .args(["serve", "--config", BASALT_CONFIG])
        .args(extra)
        .args([
            "--privacy",
            "--vision",
            "off",
            "--host",
            "127.0.0.1",
            "--unix-socket",
        ])
        .arg(&socket);
    if metrics {
        command
            .arg("--privacy-metrics-file")
            .arg(Path::new(RUNTIME).join("performance.json"));
    }
    Err(command.exec().into())
}

/// `backend health`: Docker keeps only the exit status, never a response body.
pub async fn health() -> bool {
    let runtime = Path::new(RUNTIME);
    let control = runtime.join("control.sock");
    let (socket, route) = if control.exists() {
        (control, "/v1/link/models")
    } else {
        (runtime.join("basalt.sock"), "/health")
    };
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .unix_socket(socket.as_path())
        .timeout(std::time::Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(format!("http://localhost{route}"))
        .send()
        .await
        .is_ok_and(|response| response.status() == reqwest::StatusCode::OK)
}
