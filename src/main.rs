use anyhow::Result;
use clap::{Parser, Subcommand};
use k_lani_link::{
    config::{
        Config, IceServer, RelayPolicy, SignalingBackend, configure_endpoint, generate_pair,
        loose_permissions, private_file, random_secret,
    },
    diagnostic::{self, Diagnostic},
    gateway::Gateway,
    host, shutdown, signal,
};
use std::{
    future::IntoFuture,
    io::{IsTerminal, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    version,
    about = "Private OpenAI-compatible streaming over authenticated WebRTC or an explicit HTTPS mailbox"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Run the bounded, ciphertext-only HTTPS mailbox relay.
    HttpsRelay {
        #[arg(long)]
        config: PathBuf,
    },
    /// Issue portable host/client HTTPS state folders. Copy only the client folder to the client.
    InitHttps {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        relay_url: String,
        /// Relay admission token for the host folder, and for the client folder too unless
        /// `--client-admission-token-file` names a separate one.
        #[arg(long)]
        admission_token_file: PathBuf,
        /// Separate token for the client folder, e.g. a second token in the same relay space.
        #[arg(long)]
        client_admission_token_file: Option<PathBuf>,
        #[arg(long)]
        ca_cert_file: Option<PathBuf>,
        #[arg(long)]
        proxy_url: Option<String>,
        #[arg(long, default_value_t = 86_400)]
        unused_seconds: u64,
        #[arg(long, default_value_t = 2_592_000)]
        idle_seconds: u64,
        #[arg(long)]
        absolute_seconds: Option<u64>,
        /// Where the host forwards requests (host folder only).
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        upstream: String,
        /// Reach the backend through this Unix socket instead, e.g. a container without network.
        #[arg(long)]
        upstream_socket: Option<PathBuf>,
    },
    /// Issue an HTTPS pairing and hand the client side over by a short code typed on the
    /// other computer. Waits up to ten minutes, then the code is void.
    OfferHttps {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        relay_url: String,
        /// The host's relay admission token.
        #[arg(long)]
        admission_token_file: PathBuf,
        /// The token the client receives; best a separate one in the same relay space, so a
        /// lost laptop can be revoked alone. Omitted: the client gets the host's token.
        #[arg(long)]
        client_admission_token_file: Option<PathBuf>,
        #[arg(long)]
        ca_cert_file: Option<PathBuf>,
        #[arg(long)]
        proxy_url: Option<String>,
        #[arg(long, default_value_t = 86_400)]
        unused_seconds: u64,
        #[arg(long, default_value_t = 2_592_000)]
        idle_seconds: u64,
        #[arg(long)]
        absolute_seconds: Option<u64>,
        /// Where the host forwards requests (host folder only).
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        upstream: String,
        /// Reach the backend through this Unix socket instead, e.g. a container without network.
        #[arg(long)]
        upstream_socket: Option<PathBuf>,
    },
    /// Receive the client side of an HTTPS pairing with the code the host shows.
    JoinHttps {
        /// New private folder for the client state and client.toml.
        #[arg(long)]
        dir: PathBuf,
        /// Omitted: the relay this build was made for, if any.
        #[arg(long)]
        relay_url: Option<String>,
        /// Omitted: ask in the terminal, or read one line from standard input.
        #[arg(long)]
        code: Option<String>,
        #[arg(long)]
        ca_cert_file: Option<PathBuf>,
        /// Also from `LINK_PROXY_URL`, so a proxy password stays out of the command line.
        #[arg(long, env = "LINK_PROXY_URL", hide_env_values = true)]
        proxy_url: Option<String>,
    },
    /// Revoke stopped HTTPS endpoint state; never deletes it or restores the initial code.
    RevokeHttps {
        #[arg(long)]
        config: PathBuf,
        #[arg(long,value_parser=["host","client"])]
        role: String,
    },
    /// Open the local connection control panel in your browser (also the default on double-click).
    Ui {
        /// Start without opening a browser; the private address file stays in the data directory.
        #[arg(long)]
        no_open: bool,
        /// Use a different private control-panel directory.
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Generate a new pair directory with random credentials. Keep these files private.
    Init {
        #[arg(long, default_value = "pair")]
        dir: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:8788")]
        signaling_url: String,
        #[arg(long, value_enum, default_value = "mailbox")]
        signaling_backend: SignalingBackend,
        /// Explicit STUN URI; repeat for more than one. No defaults or fallback.
        #[arg(long)]
        stun: Vec<String>,
    },
    /// Write a new random 256-bit connection code to a private file; never print it.
    NewCode {
        #[arg(long)]
        out: PathBuf,
    },
    /// Configure one node with the shared code; its first 12 characters determine the room.
    Pair {
        #[arg(long)]
        config: PathBuf,
        /// Read a private code file; omitted: ask in the terminal without echo.
        #[arg(long)]
        code_file: Option<PathBuf>,
        #[arg(long)]
        signaling_url: String,
        #[arg(long, value_enum, default_value = "ice-exchange")]
        signaling_backend: SignalingBackend,
        /// Required only for the optional private mailbox signaling backend.
        #[arg(long)]
        signaling_token_file: Option<PathBuf>,
        #[arg(long)]
        stun: Vec<String>,
        #[arg(long)]
        upstream_socket: Option<PathBuf>,
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Run an SDP-only rendezvous server. Use a TLS reverse proxy for remote access.
    Signal {
        #[arg(long, default_value = "127.0.0.1:8788")]
        listen: SocketAddr,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Run beside the configured AI backend on the other computer.
    Host {
        #[arg(long)]
        config: PathBuf,
    },
    /// Expose a loopback HTTP API to OpenCode, Cline, and other clients.
    Client {
        #[arg(long)]
        config: PathBuf,
        /// Serve only on this private Unix socket instead of a TCP listener.
        #[cfg(unix)]
        #[arg(long)]
        api_socket: Option<PathBuf>,
    },
    /// List allowlisted backend models through the current tunnel (operator credential).
    Models {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        api_socket: Option<PathBuf>,
    },
    /// Load or unload a backend model without reconnecting the encrypted tunnel.
    Model {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        api_socket: Option<PathBuf>,
        #[arg(long, required_unless_present = "unload", conflicts_with = "unload")]
        load: Option<String>,
        #[arg(long)]
        unload: bool,
    },
    /// Create a Codex provider profile and model catalog, without embedding credentials.
    CodexProfile {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        model: String,
        #[arg(long, default_value_t = 131072)]
        context: u32,
    },
    /// Credential helper for Codex. Refuses to print the token to an interactive terminal.
    CodexToken {
        #[arg(long)]
        config: PathBuf,
    },
    /// Validate a configuration without printing credentials or contacting servers.
    Check {
        #[arg(long)]
        config: PathBuf,
    },
}
fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    if std::env::args_os().len() == 1 {
        use std::os::windows::process::CommandExt;
        // A double-click launches the browser panel without leaving an empty console.
        // Explicit CLI commands retain their normal terminal and exit-code behavior.
        let launched = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg("ui")
                .creation_flags(0x08000000)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
        });
        return if launched.is_ok() {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    // Operational failures are exit codes / API responses, never log records. Command-line
    // mistakes and startup problems with a fixed, secret-free text (see `diagnostic`) are
    // reported once on stderr; connection, transport and I/O errors stay silent.
    std::panic::set_hook(Box::new(|_| {}));
    let cli = match if std::env::args_os().len() == 1 {
        Ok(Cli {
            command: Command::Ui {
                no_open: false,
                data_dir: None,
            },
        })
    } else {
        Cli::try_parse()
    } {
        Ok(cli) => cli,
        Err(error) => {
            use clap::error::ErrorKind;
            return match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    let _ = error.print();
                    std::process::ExitCode::SUCCESS
                }
                // A bare invocation: the usual help text contains only our own definitions.
                ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    let _ = error.print();
                    std::process::ExitCode::from(2)
                }
                _ => {
                    let _ = writeln!(
                        std::io::stderr(),
                        "error: {}; run k-lani-link --help for usage",
                        usage_problem(&error)
                    );
                    std::process::ExitCode::from(2)
                }
            };
        }
    };
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|runtime| runtime.block_on(run(cli)));
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if let Some(message) = diagnostic::printable(&error) {
                let _ = writeln!(std::io::stderr(), "error: {message}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

/// Clap's own error text quotes the raw arguments, which may hold a pasted credential or a
/// private path. This names the problem and, where it helps, the argument's *name* from our
/// own definitions; a supplied value or an unknown argument is never repeated.
fn usage_problem(error: &clap::Error) -> String {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    let context = |kind| match error.get(kind) {
        Some(ContextValue::String(text)) => Some(text.clone()),
        Some(ContextValue::Strings(items)) => Some(items.join(", ")),
        _ => None,
    };
    let with_argument = |problem: &str| match context(ContextKind::InvalidArg) {
        Some(argument) => format!("{problem}: {argument}"),
        None => problem.to_owned(),
    };
    match error.kind() {
        ErrorKind::InvalidSubcommand => "unknown command".into(),
        ErrorKind::UnknownArgument => "unknown argument".into(),
        ErrorKind::MissingSubcommand => "missing command".into(),
        ErrorKind::MissingRequiredArgument => with_argument("missing required argument"),
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => {
            let problem = with_argument("invalid or missing value for");
            match context(ContextKind::ValidValue) {
                Some(allowed) => format!("{problem} (allowed: {allowed})"),
                None => problem,
            }
        }
        _ => "invalid command line".into(),
    }
}

/// For commands whose errors cannot contain secrets or connection details: show them.
fn local<T>(result: Result<T>) -> Result<T> {
    result.map_err(|error| Diagnostic::err(format!("{error:#}")))
}

fn warn(message: &str) {
    let _ = writeln!(std::io::stderr(), "warning: {message}");
}

fn warn_if_loose(path: &Path) {
    if loose_permissions(path) {
        warn("configuration file is accessible by group or others; restrict it with chmod 600");
    }
}

fn warn_if_unverified(config: &Config) {
    if config.allow_unverified_backend {
        warn(
            "allow_unverified_backend is set: the backend's privacy profile is NOT checked \
             before prompts are forwarded",
        );
    }
    for backend in &config.backends {
        if backend.allow_unverified_backend {
            warn(&format!(
                "backend {} is unverified: its privacy profile is NOT checked before prompts are forwarded",
                backend.upstream
            ));
        }
        if url::Url::parse(&backend.upstream)
            .is_ok_and(|url| url.scheme() == "http" && !k_lani_link::config::loopback_url(&url))
        {
            warn(&format!(
                "backend {} is reached over plain HTTP: prompts cross that network unencrypted",
                backend.upstream
            ));
        }
    }
}

fn read_secret_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|_| Diagnostic::err("cannot read the given code or token file"))
}

async fn run(cli: Cli) -> Result<()> {
    let stop = CancellationToken::new();
    let interrupt = stop.clone();
    tokio::spawn(async move {
        k_lani_link::shutdown::signal().await;
        interrupt.cancel();
    });
    match cli.command {
        Command::HttpsRelay { config } => {
            let settings = k_lani_link::mailbox::server::Settings::load(&config)?;
            k_lani_link::mailbox::server::run(settings, stop).await?;
        }
        Command::InitHttps {
            dir,
            relay_url,
            admission_token_file,
            client_admission_token_file,
            ca_cert_file,
            proxy_url,
            unused_seconds,
            idle_seconds,
            absolute_seconds,
            upstream,
            upstream_socket,
        } => {
            let cwd = std::env::current_dir()?;
            let options = k_lani_link::mailbox::Options {
                relays: vec![relay_url],
                state_dir: cwd.join("unused"),
                admission_token_file: cwd.join(admission_token_file),
                ca_cert_file: ca_cert_file.map(|p| cwd.join(p)),
                proxy_url,
                poll_wait_seconds: 25,
                http1_only: false,
                http3: false,
                post_writes: false,
            };
            k_lani_link::mailbox::setup::issue(
                &dir,
                options,
                client_admission_token_file
                    .map(|path| cwd.join(path))
                    .as_deref(),
                k_lani_link::chain_state::Lifetimes {
                    unused_seconds,
                    idle_seconds,
                    absolute_seconds,
                },
                &k_lani_link::mailbox::setup::Upstream {
                    url: upstream,
                    socket: upstream_socket,
                },
            )?;
        }
        Command::OfferHttps {
            dir,
            relay_url,
            admission_token_file,
            client_admission_token_file,
            ca_cert_file,
            proxy_url,
            unused_seconds,
            idle_seconds,
            absolute_seconds,
            upstream,
            upstream_socket,
        } => {
            let cwd = std::env::current_dir()?;
            let options = k_lani_link::mailbox::Options {
                relays: vec![relay_url],
                state_dir: cwd.join("unused"),
                admission_token_file: cwd.join(&admission_token_file),
                ca_cert_file: ca_cert_file.map(|p| cwd.join(p)),
                proxy_url,
                poll_wait_seconds: 25,
                http1_only: false,
                http3: false,
                post_writes: false,
            };
            let client_token = k_lani_link::mailbox::secret(
                &cwd.join(client_admission_token_file.unwrap_or(admission_token_file)),
            )?;
            let host = cwd.join(&dir).join("host").join("host.toml");
            k_lani_link::mailbox::couple::offer(
                &dir,
                options,
                &client_token,
                k_lani_link::chain_state::Lifetimes {
                    unused_seconds,
                    idle_seconds,
                    absolute_seconds,
                },
                &k_lani_link::mailbox::setup::Upstream {
                    url: upstream,
                    socket: upstream_socket,
                },
                |code| {
                    println!("Pairing code: {code}");
                    println!("Enter it on the other computer within ten minutes.");
                },
            )
            .await?;
            println!(
                "Paired. Start the host with: k-lani-link host --config {}",
                host.display()
            );
        }
        Command::JoinHttps {
            dir,
            relay_url,
            code,
            ca_cert_file,
            proxy_url,
        } => {
            let relay = relay_url
                .or_else(|| k_lani_link::mailbox::couple::DEFAULT_RELAY.map(str::to_owned))
                .ok_or_else(|| Diagnostic::err("name the relay with --relay-url"))?;
            let code = match code {
                Some(code) => code,
                None => {
                    // Piped, e.g. from a launcher: no prompt, the code stays off the command line.
                    if std::io::stdin().is_terminal() {
                        print!("Pairing code: ");
                        std::io::Write::flush(&mut std::io::stdout())?;
                    }
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line.trim().to_owned()
                }
            };
            let cwd = std::env::current_dir()?;
            let config = k_lani_link::mailbox::couple::join(
                &dir,
                &k_lani_link::mailbox::couple::Code::parse(&code)?,
                k_lani_link::mailbox::couple::Join {
                    relay: &relay,
                    // An empty variable, as a launcher passes it for "no proxy", means none.
                    proxy_url: proxy_url.filter(|url| !url.is_empty()),
                    ca_cert_file: ca_cert_file.map(|p| cwd.join(p)),
                },
            )
            .await?;
            println!(
                "Paired. Start the client with: k-lani-link client --config {}",
                config.display()
            );
        }
        Command::RevokeHttps { config, role } => {
            let config = Config::load(&config)?;
            let options = config
                .https
                .as_ref()
                .filter(|_| config.transport == k_lani_link::config::Transport::Https)
                .ok_or_else(|| {
                    Diagnostic::err("HTTPS configuration required for state revocation")
                })?;
            let role = if role == "host" {
                k_lani_link_protocol::EndpointRole::Host
            } else {
                k_lani_link_protocol::EndpointRole::Client
            };
            let file = match role {
                k_lani_link_protocol::EndpointRole::Host => "host.chain",
                k_lani_link_protocol::EndpointRole::Client => "client.chain",
            };
            if !options.state_dir.join(file).is_file() {
                return Err(Diagnostic::err(
                    "selected HTTPS endpoint state is missing; verify the role",
                ));
            }
            let mut store = k_lani_link::chain_state::Store::open(&options.state_dir, role)?;
            store.revoke()?;
        }
        Command::Ui { no_open, data_dir } => {
            k_lani_link::ui::run(
                data_dir.map_or_else(k_lani_link::ui::default_directory, Ok)?,
                stop,
                !no_open,
            )
            .await?;
        }
        Command::Init {
            dir,
            signaling_url,
            signaling_backend,
            stun,
        } => {
            local(generate_pair(&dir, &signaling_url, signaling_backend, stun))?;
        }
        Command::NewCode { out } => {
            local(random_secret().and_then(|code| private_file(&out, &code)))?
        }
        Command::Pair {
            config,
            code_file,
            signaling_url,
            signaling_backend,
            signaling_token_file,
            stun,
            upstream_socket,
            state_dir,
        } => {
            let code = if let Some(path) = code_file {
                read_secret_file(&path)?
            } else {
                if !std::io::stdin().is_terminal() {
                    return Err(Diagnostic::err("use a terminal or --code-file"));
                }
                local(
                    rpassword::prompt_password("Connection code (hidden): ")
                        .map(|code| code.trim().to_owned())
                        .map_err(anyhow::Error::from),
                )?
            };
            let signaling_token = match signaling_token_file {
                Some(path) => read_secret_file(&path)?,
                None => String::new(),
            };
            local(random_secret().and_then(|api_key| {
                configure_endpoint(
                    &config,
                    Config {
                        transport: Default::default(),
                        https: None,
                        signaling_url,
                        signaling_backend,
                        signaling_token,
                        room: k_lani_link_protocol::room_from_key(&code).map_err(|_| {
                            Diagnostic::err(
                                "the connection code must have 43 URL-safe base64 characters",
                            )
                        })?,
                        pair_key: code,
                        pairing_state_dir: state_dir,
                        api_key,
                        listen: "127.0.0.1:8787".parse()?,
                        upstream: "http://localhost:8080".into(),
                        upstream_socket,
                        upstream_api_key: String::new(),
                        relay: RelayPolicy::Disabled,
                        ice_servers: stun
                            .into_iter()
                            .map(|url| IceServer {
                                urls: vec![url],
                                ..Default::default()
                            })
                            .collect(),
                        request_timeout_seconds: 600,
                        allow_unverified_backend: false,
                        include_loopback_candidate: false,
                        resume_seconds: 0,
                        backends: vec![],
                        max_concurrent_requests: 1,
                        max_queued_requests: 0,
                        management_token_file: None,
                        model_control_socket: None,
                    },
                )
            }))?;
        }
        Command::Models { config, api_socket } => {
            let cfg = Config::load(&config)?;
            let result =
                k_lani_link::management::request(&cfg, api_socket.as_deref(), None).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Model {
            config,
            api_socket,
            load,
            unload: _,
        } => {
            let cfg = Config::load(&config)?;
            let command = load.map_or(k_lani_link_protocol::ModelCommand::Unload {}, |model| {
                k_lani_link_protocol::ModelCommand::Load { model }
            });
            let result =
                k_lani_link::management::request(&cfg, api_socket.as_deref(), Some(command))
                    .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Check { config } => {
            let loaded = Config::load(&config)?;
            let _ = k_lani_link::management::token(&loaded)?;
            warn_if_loose(&config);
            warn_if_unverified(&loaded);
            // Explicitly requested by the operator, like --help.
            println!("configuration OK");
        }
        Command::CodexProfile {
            config,
            output,
            model,
            context,
        } => {
            local(k_lani_link::codex::profile(
                &config, &output, &model, context,
            ))?;
        }
        Command::CodexToken { config } => local(k_lani_link::codex::token(&config))?,
        Command::Signal { listen, token_file } => {
            let token = read_secret_file(&token_file)?;
            if token.len() < 32 {
                return Err(Diagnostic::err(
                    "signaling token must have at least 32 characters",
                ));
            }
            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .map_err(|error| {
                    Diagnostic::err(format!("cannot listen on the given address: {error}"))
                })?;
            axum::serve(listener, signal::router(token))
                .with_graceful_shutdown(stop.cancelled_owned())
                .await?;
        }
        Command::Host { config } => {
            let loaded = Config::load(&config)?;
            warn_if_loose(&config);
            warn_if_unverified(&loaded);
            let state_dir = k_lani_link::pairing::state_directory(&loaded, &config);
            host::run(loaded, stop, state_dir).await?;
        }
        Command::Client {
            config,
            #[cfg(unix)]
            api_socket,
        } => {
            let loaded = Config::load(&config)?;
            warn_if_loose(&config);
            let state_dir = k_lani_link::pairing::state_directory(&loaded, &config);
            let config = loaded;
            #[cfg(unix)]
            let unix = api_socket
                .map(|path| {
                    k_lani_link::local_api::Socket::bind(&path).map_err(|error| {
                        Diagnostic::err(format!("cannot create the API socket: {error}"))
                    })
                })
                .transpose()?;
            #[cfg(unix)]
            let use_tcp = unix.is_none();
            #[cfg(not(unix))]
            let use_tcp = true;
            let tcp = if use_tcp {
                Some(
                    tokio::net::TcpListener::bind(config.listen)
                        .await
                        .map_err(|error| {
                            Diagnostic::err(format!(
                                "cannot listen on the configured address: {error}"
                            ))
                        })?,
                )
            } else {
                None
            };
            let gateway = Gateway::new(config)?;
            let background = gateway.clone();
            let shutdown = stop.clone();
            let task = tokio::spawn(async move {
                background.maintain(shutdown, state_dir).await;
            });
            // After a stop request, running requests get a few seconds, not the whole generation.
            if let Some(listener) = tcp {
                let server = axum::serve(listener, gateway.router())
                    .with_graceful_shutdown(stop.clone().cancelled_owned());
                if let Some(result) =
                    shutdown::with_grace(&stop, shutdown::GRACE, server.into_future()).await
                {
                    result?;
                }
            }
            #[cfg(unix)]
            if let Some((_socket, listener)) = unix {
                let server = axum::serve(listener, gateway.router())
                    .with_graceful_shutdown(stop.clone().cancelled_owned());
                if let Some(result) =
                    shutdown::with_grace(&stop, shutdown::GRACE, server.into_future()).await
                {
                    result?;
                }
            }
            stop.cancel();
            task.await?;
        }
    }
    Ok(())
}
