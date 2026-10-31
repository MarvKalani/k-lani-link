//! Synthetic load against the actual host, gateway and HTTPS transport; never a model benchmark.
use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    response::Response,
    routing::{get, post},
};
use clap::Parser;
use futures_util::StreamExt;
use k_lani_link::{
    chain_state::Lifetimes,
    config::{Config, private_file},
    gateway::Gateway,
    mailbox::{Options, setup},
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::TcpListener, task::JoinSet, time::Instant};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    relay: String,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    ca: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    out: PathBuf,
    /// Lab-only start gate lets the runner shape this container's network before traffic.
    #[arg(long)]
    start_file: Option<PathBuf>,
    #[arg(long, default_value_t = 50)]
    rate: u64,
    #[arg(long, default_value_t = 1)]
    tunnels: usize,
    #[arg(long, default_value_t = 60)]
    seconds: u64,
    /// Offer only HTTP/1.1 instead of negotiating HTTP/2.
    #[arg(long)]
    http1_only: bool,
    /// Try HTTP/3 over QUIC first (needs UDP to the relay).
    #[arg(long)]
    http3: bool,
    /// Send writes as POST from the start (PHP relays behind PUT-blocking firewalls).
    #[arg(long)]
    post_writes: bool,
    /// Seconds a read may wait at the relay (0-25).
    #[arg(long, default_value_t = 25)]
    poll_wait: u64,
}
struct Backend {
    epoch: Instant,
    rate: u64,
    seconds: u64,
}
async fn completion(State(backend): State<Arc<Backend>>) -> Response {
    let start = Instant::now();
    let stream = async_stream::stream! {
        for n in 0..backend.rate*backend.seconds {
            let scheduled = start + Duration::from_nanos(n*1_000_000_000/backend.rate);
            tokio::time::sleep_until(scheduled).await;
            let stamp = scheduled.duration_since(backend.epoch).as_micros() as u64;
            let mut event = format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"12345678\"}}}}],\"n\":{n},\"scheduled_us\":{stamp}}}");
            // Exactly the 161-byte SSE load used by the carrier decision experiment.
            event.extend(std::iter::repeat_n(' ', 159usize.saturating_sub(event.len())));
            event.push_str("\n\n");
            yield Ok::<_, std::convert::Infallible>(Bytes::from(event));
        }
        yield Ok(Bytes::from_static(b"data: [DONE]\n\n"));
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
async fn consume(
    http: reqwest::Client,
    api: String,
    key: String,
    backend: Arc<Backend>,
    began: Instant,
) -> Result<(Vec<u64>, u64, bool)> {
    let mut buffer = Vec::new();
    let mut delays = Vec::new();
    let mut first = 0;
    let mut done = false;
    let result = async {
    let mut response = http.post(format!("{api}/v1/chat/completions")).bearer_auth(key)
        .json(&serde_json::json!({"model":"synthetic","stream":true,"store":false,"messages":[{"role":"user","content":"synthetic"}]})).send().await?.error_for_status()?.bytes_stream();
    while let Some(chunk) = response.next().await {
        buffer.extend_from_slice(&chunk?);
        while let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
            let event = buffer.drain(..end + 2).collect::<Vec<_>>();
            let data = std::str::from_utf8(&event)?
                .trim()
                .strip_prefix("data: ")
                .ok_or_else(|| anyhow::anyhow!("invalid synthetic event"))?;
            if data == "[DONE]" {
                ensure!(!done, "duplicate end");
                done = true;
                continue;
            }
            ensure!(!done, "event after end");
            let value: serde_json::Value = serde_json::from_str(data)?;
            ensure!(
                value["n"].as_u64() == Some(delays.len() as u64),
                "out-of-order synthetic event"
            );
            let scheduled = value["scheduled_us"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("missing synthetic timestamp"))?;
            if delays.is_empty() {
                first = began.elapsed().as_micros() as u64;
            }
            delays.push((backend.epoch.elapsed().as_micros() as u64).saturating_sub(scheduled));
        }
    }
    ensure!(
        done && buffer.is_empty() && delays.len() as u64 == backend.rate * backend.seconds,
        "incomplete synthetic stream"
    );
    Ok::<_,anyhow::Error>(())
    }.await;
    Ok((delays, first, result.is_ok()))
}
async fn run(args: Args, phase: &mut &'static str) -> Result<()> {
    ensure!(
        matches!(args.rate, 50 | 100)
            && (1..=10).contains(&args.tunnels)
            && (1..=60).contains(&args.seconds),
        "invalid fixed workload"
    );
    if let Some(path) = &args.start_file {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !path.is_file() {
            ensure!(Instant::now() < deadline, "lab start deadline");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    *phase = "starting local synthetic backend";
    let backend = Arc::new(Backend {
        epoch: Instant::now(),
        rate: args.rate,
        seconds: args.seconds,
    });
    let began = Instant::now();
    let stop = CancellationToken::new();
    let mut services = JoinSet::new();
    let router=Router::new().route("/health",get(||async{Json(serde_json::json!({"link_privacy":{"version":1,"application_logs":false,"request_history":false,"diagnostics":false,"remote_images":false}}))}))
        .route("/v1/chat/completions",post(completion)).with_state(backend.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let upstream = format!("http://{}", listener.local_addr()?);
    let stopping = stop.clone();
    services.spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopping.cancelled_owned())
            .await
            .map_err(anyhow::Error::from)
    });
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(args.seconds + 30))
        .build()?;
    let mut consumers = JoinSet::new();
    for index in 0..args.tunnels {
        *phase = "issuing private endpoint state";
        let options = Options {
            relays: vec![args.relay.clone()],
            state_dir: args.state.join("unused"),
            admission_token_file: args.token_file.clone(),
            ca_cert_file: Some(args.ca.clone()),
            proxy_url: None,
            poll_wait_seconds: args.poll_wait,
            http1_only: args.http1_only,
            http3: args.http3,
            post_writes: args.post_writes,
        };
        let pair = args.state.join(format!("pair-{index}"));
        setup::issue(
            &pair,
            options,
            None,
            Lifetimes::default(),
            &Default::default(),
        )?;
        let mut host = Config::load(&pair.join("host/host.toml"))?;
        host.upstream = upstream.clone();
        let client = Config::load(&pair.join("client/client.toml"))?;
        let key = client.api_key.clone();
        let gateway = Gateway::new(client)?;
        let serving = gateway.clone();
        let stopping = stop.clone();
        services.spawn(async move {
            serving.maintain(stopping, PathBuf::new()).await;
            Ok(())
        });
        let stopping = stop.clone();
        services.spawn(async move { k_lani_link::host::run(host, stopping, PathBuf::new()).await });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let api = format!("http://{}", listener.local_addr()?);
        let router = gateway.router();
        let stopping = stop.clone();
        services.spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(stopping.cancelled_owned())
                .await
                .map_err(anyhow::Error::from)
        });
        let http = http.clone();
        let backend = backend.clone();
        consumers.spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !gateway.is_connected() {
                ensure!(Instant::now() < deadline, "HTTPS setup deadline");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            consume(http, api, key, backend, began).await
        });
    }
    let mut delays = Vec::new();
    let mut first = 0;
    let mut failed = 0;
    *phase = "negotiating or consuming authenticated streams";
    while let Some(result) = consumers.join_next().await {
        let (mut measured, first_token, complete) = result??;
        failed += usize::from(!complete);
        first = first.max(first_token);
        delays.append(&mut measured);
    }
    let wall = began.elapsed().as_secs_f64();
    stop.cancel();
    *phase = "stopping isolated endpoints";
    while let Some(result) = services.join_next().await {
        result??;
    }
    delays.sort_unstable();
    private_file(
        &args.out,
        &serde_json::to_string_pretty(&serde_json::json!({
            "carrier":"acknowledged-mailbox","implementation":"actual-host-gateway-https","synthetic":true,
            "tokens_per_second_per_tunnel":args.rate,"tunnels":args.tunnels,"active_seconds":args.seconds,
            "offered":args.rate*args.seconds*args.tunnels as u64,"received":delays.len(),"authenticated_completion":failed==0,"failed_tunnels":failed,
            "first_token_max_ms":first as f64/1000.0,"delay_p50_ms":delays.get(delays.len()/2).map(|v|*v as f64/1000.0),
            "delay_p99_ms":delays.get(delays.len()*99/100).map(|v|*v as f64/1000.0),"wall_seconds":wall,"received_per_second":delays.len() as f64/wall,
            "drain_deadline_seconds":30,"failure_category":if failed>0{Some("incomplete_or_invalid_stream_within_deadline")}else{None}
        }))?,
    )?;
    // Keep cgroups alive briefly so the runner reads final counters before cleanup.
    if let Some(path) = &args.start_file {
        let deadline = Instant::now() + Duration::from_secs(10);
        while path.is_file() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    std::panic::set_hook(Box::new(|_| {}));
    let Ok(args) = Args::try_parse() else {
        return std::process::ExitCode::from(2);
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
    else {
        return std::process::ExitCode::FAILURE;
    };
    let output = args.out.clone();
    let mut phase = "validating synthetic workload";
    if runtime.block_on(run(args, &mut phase)).is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        let _ = private_file(&output, &serde_json::json!({"failure":phase}).to_string());
        std::process::ExitCode::FAILURE
    }
}
