"""Run synthetic, encrypted carrier tests in a private Docker network.

Never contacts the production VPS, mounts runtime keys, or changes host qdiscs.
Build dependencies can use the Internet; test containers have an internal network.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
SOURCE = Path(__file__).resolve().parent
IMAGE = "k-lani-link-mailbox-bench:local"
PROFILES = {"small": (2, 1536 * 1024**2), "large": (4, 7168 * 1024**2)}
MODES = ["mailbox", "stream", "post-stream", "sse", "longpoll", "h2-stream", "wss"]


def command(*args, timeout=180):
    return subprocess.run(args, cwd=ROOT, check=True, capture_output=True,
                          text=True, timeout=timeout).stdout.strip()


def remove(*args):
    subprocess.run(args, cwd=ROOT, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, timeout=30, check=False)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=["small", "large", "both"], default="both")
    parser.add_argument("--seconds", type=int, default=6)
    parser.add_argument("--rtts", default="0,50,150", help="Endpoint-to-relay RTTs in ms")
    parser.add_argument("--workloads", default="50:1,1000:1,100:10,10:100", help="Tokens/s per tunnel:number of tunnels")
    parser.add_argument("--mbit", type=int, default=500)
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--tls", choices=["direct", "nginx", "nginx-upload-buffered", "nginx-response-buffered", "nginx-no-upgrade"], default="direct")
    parser.add_argument("--modes", default=",".join(MODES))
    parser.add_argument("--jitter-ms", type=int, default=0)
    parser.add_argument("--loss-percent", type=float, default=0)
    parser.add_argument("--drain-seconds", type=int, default=10)
    parser.add_argument("--idle-seconds", type=int, default=0)
    parser.add_argument("--cut-after-records", type=int, default=0)
    parser.add_argument("--server-cpuset", default="")
    parser.add_argument("--client-cpuset", default="")
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if len(Path("/proc/swaps").read_text().splitlines()) > 1:
        parser.error("Disable swap on the benchmark host first; this script never changes host swap.")
    rtts = [int(value) for value in args.rtts.split(",")]
    workloads = [tuple(map(int, value.split(":"))) for value in args.workloads.split(",")]
    modes = args.modes.split(",")
    if not (1 <= args.seconds <= 60 and 1 <= args.repeat <= 10 and 1 <= args.mbit <= 10000
            and all(mode in MODES for mode in modes) and len(set(modes)) == len(modes)
            and 0 <= args.jitter_ms <= 100 and 0 <= args.loss_percent <= 5 and 1 <= args.drain_seconds <= 60
            and 0 <= args.idle_seconds <= 120 and 0 <= args.cut_after_records <= 10000
            and all(2 * args.jitter_ms <= rtt <= 1000 for rtt in rtts)
            and all(1 <= rate <= 1000 and 1 <= count <= 100 and rate * count <= 1000 for rate, count in workloads)):
        parser.error("Workload outside the bounded synthetic benchmark range.")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = ROOT / ".release" / ("mailbox-bench-" + stamp + "-" + uuid.uuid4().hex[:8])
    output.mkdir(mode=0o700, parents=True)
    target = ROOT / ".release/mailbox-review-866d456/target"
    if not args.skip_build:
        print("Building isolated benchmark and instrumentation image.", flush=True)
        env = dict(os.environ, CARGO_TARGET_DIR=str(target))
        with (output / "build.txt").open("w") as log:
            subprocess.run(["cargo", "build", "--locked", "--release", "--jobs", "2", "--manifest-path", str(SOURCE / "Cargo.toml")],
                           cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        context = output / "build-context"
        context.mkdir()
        for name in ["Dockerfile", "nginx.conf", "entrypoint.sh"]:
            shutil.copy2(SOURCE / name, context / name)
        shutil.copy2(target / "release/link-mailbox-bench", context / "link-mailbox-bench")
        with (output / "docker-build.txt").open("w") as log:
            subprocess.run(["docker", "build", "--tag", IMAGE, str(context)], cwd=ROOT,
                           stdout=log, stderr=subprocess.STDOUT, check=True)
    certs = output / "cert"
    certs.mkdir(mode=0o755)
    command("openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
            "-days", "1", "-subj", "/CN=synthetic-benchmark-ca", "-addext", "basicConstraints=critical,CA:TRUE",
            "-keyout", str(certs / "ca.key"), "-out", str(certs / "ca.crt"))
    command("openssl", "req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
            "-subj", "/CN=bench-relay", "-keyout", str(certs / "server.key"), "-out", str(certs / "server.csr"))
    (certs / "extensions.txt").write_text("subjectAltName=DNS:bench-relay\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
    command("openssl", "x509", "-req", "-in", str(certs / "server.csr"), "-CA", str(certs / "ca.crt"),
            "-CAkey", str(certs / "ca.key"), "-CAcreateserial", "-days", "1", "-extfile", str(certs / "extensions.txt"), "-out", str(certs / "server.crt"))
    for name in ["ca.key", "ca.srl", "server.csr", "extensions.txt"]:
        (certs / name).unlink()
    (certs / "server.key").chmod(0o644)  # Disposable synthetic key inside a private parent directory.
    os_release = platform.freedesktop_os_release()
    metadata = {
        "date_utc": stamp, "commit": command("git", "rev-parse", "HEAD"),
        "kind": "local resource-limited synthetic carrier comparison; not a WAN or LLM benchmark",
        "host_cpu": next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
        "host_os": os_release.get("PRETTY_NAME"), "kernel": platform.release(),
        "docker": command("docker", "version", "--format", "{{.Server.Version}}"),
        "image": json.loads(command("docker", "image", "inspect", IMAGE))[0]["Id"],
        "container_os": "Debian 13 (trixie)", "swap": "off",
        "reference_small": {"provider": "small VPS (owner supplied)", "cpu": "AMD EPYC (SSH observed)", "vcpus": 2, "total_ram_bytes": 1957814272},
        "reference_large": {"source": "typical VPS plan", "plan": "4 vCPU / 8 GB, not owner-confirmed", "vcpus": 4, "ram_gb": 8},
        "network": {"mbit_per_second_per_container_egress": args.mbit, "endpoint_to_relay_rtt_ms": rtts, "jitter_ms_per_egress": args.jitter_ms, "loss_percent_per_egress": args.loss_percent},
        "profiles": {name: {"cpus": cpu, "ram_limit_bytes": memory} for name, (cpu, memory) in PROFILES.items()},
        "seconds": args.seconds, "repetitions": args.repeat, "tls": args.tls, "modes": modes, "drain_seconds": args.drain_seconds,
        "idle_seconds": args.idle_seconds, "cut_after_records": args.cut_after_records,
        "server_cpuset": args.server_cpuset, "client_cpuset": args.client_cpuset,
        "source_sha256": {str(path.relative_to(ROOT)): digest(path) for path in sorted(SOURCE.rglob("*")) if path.is_file() and "__pycache__" not in path.parts},
    }
    (output / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    ident = uuid.uuid4().hex[:10]
    network, server, client = ["link-bench-" + ident + suffix for suffix in ["-net", "-relay", "-client"]]
    command("docker", "network", "create", "--internal", network)
    common = ["--network", network, "--read-only", "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "--cap-add", "SETUID", "--cap-add", "SETGID", "--cap-add", "SETPCAP",
              "--security-opt", "no-new-privileges:true", "--pids-limit", "128", "--log-driver", "none", "--ulimit", "core=0:0",
              "--tmpfs", "/tmp:rw,noexec,nosuid,size=32m,uid=1000,gid=1000",
              "--env", f"BENCH_MBIT={args.mbit}", "--env", f"BENCH_TLS={args.tls}", "--env", f"BENCH_JITTER_MS={args.jitter_ms}",
              "--env", f"BENCH_LOSS_PERCENT={args.loss_percent}", "--env", f"BENCH_DRAIN_SECONDS={args.drain_seconds}",
              "--env", f"BENCH_IDLE_SECONDS={args.idle_seconds}", "--env", f"BENCH_CUT_AFTER_RECORDS={args.cut_after_records}"]
    server_options = ["--mount", f"type=bind,src={certs},dst=/cert,readonly"]
    client_options = ["--mount", f"type=bind,src={certs / 'ca.crt'},dst=/cert/ca.crt,readonly"]
    if args.server_cpuset:
        server_options.extend(["--cpuset-cpus", args.server_cpuset])
    if args.client_cpuset:
        client_options.extend(["--cpuset-cpus", args.client_cpuset])
    runtime_image = metadata["image"]
    profiles = list(PROFILES) if args.profile == "both" else [args.profile]
    try:
        with (output / "results.jsonl").open("w") as results:
            for repetition in range(args.repeat):
                for profile in profiles:
                    cpu, memory = PROFILES[profile]
                    for rtt in rtts:
                        for rate, tunnels in workloads:
                            for mode in modes:
                                print(f"{profile}: {mode}, {tunnels} x {rate} synthetic tokens/s, {rtt} ms RTT, run {repetition + 1}", flush=True)
                                delay = ["--env", f"BENCH_DELAY_MS={rtt / 2}"]
                                command("docker", "run", "-d", "--name", server, "--network-alias", "bench-relay", *common, *delay, *server_options,
                                        "--cpus", str(cpu), "--memory", str(memory), "--memory-swap", str(memory), "--env", "TOKIO_WORKER_THREADS=2", runtime_image, "serve")
                                try:
                                    for attempt in range(40):
                                        try:
                                            command("docker", "exec", server, "python", "-c", "import socket; socket.create_connection(('127.0.0.1', 8443), .2).close()", timeout=5)
                                            break
                                        except subprocess.CalledProcessError:
                                            if attempt == 39:
                                                raise
                                            time.sleep(0.1)
                                    result = json.loads(command("docker", "run", "--rm", "--name", client, *common, *delay, *client_options,
                                        "--cpus", "4", "--memory", "512m", "--memory-swap", "512m", "--env", "TOKIO_WORKER_THREADS=4",
                                        "--env", "BENCH_BASE=https://bench-relay:8443", "--env", "BENCH_CA=/cert/ca.crt",
                                        runtime_image, "run", mode, str(rate), str(args.seconds), str(tunnels), timeout=args.seconds + args.idle_seconds + 100))
                                    result.update(profile=profile, requested_rtt_ms=rtt, mbit=args.mbit, repetition=repetition + 1, tls=args.tls,
                                                  jitter_ms_per_egress=args.jitter_ms, loss_percent_per_egress=args.loss_percent)
                                    results.write(json.dumps(result, sort_keys=True) + "\n")
                                    results.flush()
                                    print(f"  received {result['received_tokens']}/{result['offered_tokens']}; failures {result['failed_tunnels']}; p99 {result['latency_p99_ms']} ms", flush=True)
                                finally:
                                    remove("docker", "rm", "-f", client, server)
    finally:
        remove("docker", "rm", "-f", client, server)
        remove("docker", "network", "rm", network)
        shutil.rmtree(certs)
    print(f"Aggregate measurements: {output}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (subprocess.SubprocessError, ValueError, OSError, KeyboardInterrupt):
        sys.exit("Benchmark setup/run failed. No production services were changed; inspect the local build report.")
