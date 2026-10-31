"""Measure only the selected, actual HTTPS stack in isolated Linux containers.

No model, production service, endpoint credential or Docker socket is mounted.
An existing instrumentation image supplies tc in disposable network helpers only.
"""
import argparse
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import platform
import secrets
import shutil
import socket
import ssl
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
SOURCE = Path(__file__).resolve().parent
BINARY = ROOT / "target/x86_64-unknown-linux-musl/release"


def command(*args, timeout=30):
    return subprocess.run(args, check=True, capture_output=True, text=True,
                          cwd=ROOT, timeout=timeout).stdout.strip()


def remove(*args):
    subprocess.run(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   cwd=ROOT, timeout=30, check=False)


def inspect(name):
    return json.loads(command("docker", "inspect", name))[0]


def cgroup(name):
    pid = inspect(name)["State"]["Pid"]
    relative = next(line.split(":", 2)[2] for line in Path(f"/proc/{pid}/cgroup").read_text().splitlines() if line.startswith("0::"))
    return Path("/sys/fs/cgroup") / relative.lstrip("/")


def counters(path):
    cpu = dict(line.split() for line in (path / "cpu.stat").read_text().splitlines())
    return int(cpu["usage_usec"]), int((path / "memory.peak").read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--profile", choices=["small", "large"], default="small")
    parser.add_argument("--workloads", default="50:1,100:1,50:10,100:10")
    parser.add_argument("--seccomp-base", type=Path, default=Path("/usr/share/containers/seccomp.json"))
    parser.add_argument("--tools-image", default="k-lani-link-mailbox-bench:local")
    parser.add_argument("--http1-only", action="store_true", help="endpoints offer only HTTP/1.1 instead of negotiating HTTP/2")
    parser.add_argument("--http3", action="store_true", help="relay serves and endpoints try HTTP/3 (QUIC) first")
    parser.add_argument("--udp", choices=["denied", "allowed"], default="denied",
                        help="denied keeps the seccomp UDP block (HTTP/3 then measures its TCP fallback)")
    parser.add_argument("--loss", type=float, default=0.0, help="netem packet loss percent on each egress (0-10)")
    args = parser.parse_args()
    workloads = [tuple(map(int, item.split(":"))) for item in args.workloads.split(",")]
    if os.getuid() != 1000 or not (1 <= args.seconds <= 60) or any(rate not in (50, 100) or count not in (1, 10) for rate, count in workloads):
        parser.error("This bounded Linux lab requires UID 1000 and 50/100 tokens/s with 1/10 tunnels.")
    if len(Path("/proc/swaps").read_text().splitlines()) > 1:
        parser.error("Disable host swap first; this runner never changes it.")
    if not 0 <= args.loss <= 10:
        parser.error("--loss must be 0..10 percent.")
    if not args.seccomp_base.is_file():
        parser.error("Supply a trusted runtime seccomp allowlist with --seccomp-base.")
    baseline = json.loads(args.seccomp_base.read_text())
    if baseline.get("defaultAction") not in ("SCMP_ACT_ERRNO", "SCMP_ACT_KILL", "SCMP_ACT_KILL_PROCESS"):
        parser.error("The baseline must deny unknown syscalls, not allow everything.")
    # Narrow existing socket allows too: libseccomp may optimize a broad ALLOW
    # before an overlapping deny in the same filter. Do not rely on rule order.
    rules = []
    for original in baseline["syscalls"] if args.udp == "denied" else []:
        if original["action"] == "SCMP_ACT_ALLOW" and "socket" in original["names"]:
            selected = json.loads(json.dumps(original))
            original = json.loads(json.dumps(original))
            original["names"].remove("socket")
            if original["names"]:
                rules.append(original)
            selected["names"] = ["socket"]
            if any(arg["index"] == 1 for arg in selected.get("args") or []):
                parser.error("Unsupported baseline socket-type condition; review it before extending.")
            # Only SOCK_STREAM is needed in this lab, with optional NONBLOCK/CLOEXEC flags.
            selected["args"] = (selected.get("args") or []) + [{"index": 1, "value": 15, "valueTwo": 1, "op": "SCMP_CMP_MASKED_EQ"}]
            rules.append(selected)
        else:
            rules.append(original)
    if args.udp == "denied":
        rules.append({"names": ["socket"], "action": "SCMP_ACT_ERRNO", "errnoRet": 13,
                      "args": [{"index": 1, "value": 15, "valueTwo": 2, "op": "SCMP_CMP_MASKED_EQ"}]})
        baseline["syscalls"] = rules
    output = ROOT / ".release" / ("https-lab-" + time.strftime("%Y%m%dT%H%M%SZ", time.gmtime()) + "-" + uuid.uuid4().hex[:8])
    output.mkdir(mode=0o700, parents=True)
    profile = output / "no-datagram.json"
    profile.write_text(json.dumps(baseline))
    context = output / "image"
    context.mkdir()
    for name, source in [("k-lani-link", BINARY / "k-lani-link"), ("https-probe", BINARY / "examples/https-probe")]:
        shutil.copy2(source, context / name)
    (context / "Dockerfile").write_text('FROM scratch\nCOPY k-lani-link https-probe /\nUSER 1000:1000\nENTRYPOINT ["/k-lani-link"]\n')
    image = "k-lani-link-https-lab:" + uuid.uuid4().hex[:8]
    with (output / "build.txt").open("w") as report:
        subprocess.run(["docker", "build", "-t", image, str(context)], stdout=report, stderr=subprocess.STDOUT, check=True)
    tools = inspect(args.tools_image)["Id"]
    image_id = inspect(image)["Id"]
    names = ["link-https-" + uuid.uuid4().hex[:10] + suffix for suffix in ("-net", "-relay", "-load")]
    network, relay, load = names
    command("docker", "network", "create", "--internal", network)
    subnet = ipaddress.ip_network(inspect(network)["IPAM"]["Config"][0]["Subnet"])
    relay_ip = str(subnet.network_address + 10)
    cert = output / "relay"
    cert.mkdir(mode=0o700)
    token = secrets.token_urlsafe(32)
    (cert / "token").write_text(token)
    (cert / "token").chmod(0o600)
    command("openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-days", "1",
            "-subj", "/CN=synthetic-private-lab-ca", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign",
            "-keyout", str(cert / "ca.key"), "-out", str(cert / "ca.pem"))
    command("openssl", "req", "-new", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
            "-subj", "/CN=synthetic-private-lab-relay", "-keyout", str(cert / "server.key"), "-out", str(cert / "server.csr"))
    (cert / "extensions.txt").write_text(f"subjectAltName=IP:{relay_ip}\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\n")
    command("openssl", "x509", "-req", "-in", str(cert / "server.csr"), "-CA", str(cert / "ca.pem"), "-CAkey", str(cert / "ca.key"),
            "-CAcreateserial", "-days", "1", "-extfile", str(cert / "extensions.txt"), "-out", str(cert / "server.crt"))
    for name in ("ca.key", "ca.srl", "server.csr", "extensions.txt"):
        (cert / name).unlink()
    (cert / "server.key").chmod(0o600)
    (cert / "relay.toml").write_text('listen = "0.0.0.0:8789"\ntls_cert = "server.crt"\ntls_key = "server.key"\n'
                                     + ("http3 = true\n" if args.http3 else "") + '[[tokens]]\ntoken_file = "token"\n')
    common = ["--network", network, "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
              "--security-opt", f"seccomp={profile}", "--pids-limit", "128", "--log-driver", "none", "--ulimit", "core=0:0",
              "--env", "HTTPS_PROXY=http://127.0.0.1:9", "--env", "HTTP_PROXY=http://127.0.0.1:9", "--env", "ALL_PROXY=http://127.0.0.1:9", "--env", "NO_PROXY="]
    cpu, memory = (2, 1536) if args.profile == "small" else (4, 7168)
    metadata = {"kind": "local synthetic actual-host/gateway HTTPS test; not WAN, EPYC, real LLM or public capacity",
                "http_versions": "HTTP/1.1 only" if args.http1_only else "ALPN h2, falling back to HTTP/1.1",
                "host_cpu": next(line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
                "host_os": platform.freedesktop_os_release().get("PRETTY_NAME"), "kernel": platform.release(),
                "docker": command("docker", "version", "--format", "{{.Server.Version}}"), "container_os": "scratch (no userspace OS)",
                "source_commit": command("git", "rev-parse", "HEAD"), "source_dirty": bool(command("git", "status", "--porcelain")),
                "build_inputs_sha256": {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml")},
                "rustc": command("rustc", "--version"),
                "host_total_ram_bytes": int(next(line.split()[1] for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemTotal:"))) * 1024,
                "source_sha256": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for tree in (ROOT / "src", ROOT / "crates", SOURCE) for path in sorted(tree.rglob("*")) if path.is_file() and "__pycache__" not in path.parts},
                "binary_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in context.iterdir() if p.name != "Dockerfile"},
                "image": image_id, "tools_image": tools, "image_bytes": inspect(image)["Size"], "profile": args.profile,
                "relay_cpu_quota": cpu, "relay_ram_mib": memory, "load_cpu_quota": 4, "load_ram_mib": 512,
                "endpoint_to_relay_rtt_ms": 50, "egress_mbit_per_container": 500, "swap": "off",
                "udp": "socket creation denied by seccomp" if args.udp == "denied" else "allowed (baseline seccomp)",
                "http3": args.http3, "packet_loss_percent": args.loss,
                "environment_proxies": "unreachable, deliberately ignored", "seccomp_base_sha256": hashlib.sha256(args.seccomp_base.read_bytes()).hexdigest(),
                "seccomp_test_sha256": hashlib.sha256(profile.read_bytes()).hexdigest()}
    results = []
    try:
        # Prove the socket filter is active (including SOCK_NONBLOCK flags) before a run.
        proof = "import socket,errno\nfor family in (socket.AF_INET,socket.AF_INET6):\n for flags in (0,socket.SOCK_NONBLOCK):\n  try: socket.socket(family,socket.SOCK_DGRAM|flags)\n  except OSError as e: assert e.errno==errno.EACCES\n  else: raise AssertionError('UDP was available')\n"
        if args.udp == "allowed":
            proof = "import socket\nsocket.socket(socket.AF_INET,socket.SOCK_DGRAM).close()\n"
        command("docker", "run", "--rm", "--network", "none", "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true", "--security-opt", f"seccomp={profile}",
                "--read-only", "--log-driver", "none", "--entrypoint", "python", tools, "-c", proof)
        for rate, tunnels in workloads:
            state = output / f"run-{rate}-{tunnels}"
            state.mkdir(mode=0o700)
            command("docker", "run", "-d", "--name", relay, *common, "--ip", relay_ip, "--cpus", str(cpu), "--memory", f"{memory}m", "--memory-swap", f"{memory}m",
                    "--env", "TOKIO_WORKER_THREADS=2", "--mount", f"type=bind,src={cert},dst=/config,readonly", image_id, "https-relay", "--config", "/config/relay.toml")
            command("docker", "run", "-d", "--name", load, *common, "--cpus", "4", "--memory", "512m", "--memory-swap", "512m", "--entrypoint", "/https-probe",
                    "--mount", f"type=bind,src={state},dst=/state", "--mount", f"type=bind,src={cert / 'token'},dst=/token,readonly",
                    "--mount", f"type=bind,src={cert / 'ca.pem'},dst=/ca.pem,readonly", image_id, "--relay", f"https://{relay_ip}:8789",
                    "--token-file", "/token", "--ca", "/ca.pem", "--state", "/state", "--out", "/state/result.json", "--start-file", "/state/start",
                    "--rate", str(rate), "--tunnels", str(tunnels), "--seconds", str(args.seconds),
                    *(["--http1-only"] if args.http1_only else []), *(["--http3"] if args.http3 else []))
            try:
                # Only helper containers have NET_ADMIN; the actual processes have zero capabilities.
                for name in (relay, load):
                    command("docker", "run", "--rm", "--network", f"container:{name}", "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "--security-opt", "no-new-privileges:true",
                            "--read-only", "--log-driver", "none", "--entrypoint", "tc", tools, "qdisc", "add", "dev", "eth0", "root", "netem", "limit", "20000", "delay", "25ms", "rate", "500mbit",
                            *(["loss", f"{args.loss}%"] if args.loss else []))
                context_tls = ssl.create_default_context(cafile=str(cert / "ca.pem"))
                with socket.create_connection((relay_ip, 8789), 3) as sock:
                    with context_tls.wrap_socket(sock, server_hostname=relay_ip) as secured:
                        secured.sendall(b"GET /invalid HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n")
                        assert secured.recv(128).startswith(b"HTTP/1.1 404")
                paths = [cgroup(name) for name in (relay, load)]
                before = [counters(path)[0] for path in paths]
                began = time.monotonic()
                (state / "start").touch()
                print(f"{args.profile}: {tunnels} x {rate} synthetic tokens/s for {args.seconds}s; UDP {args.udp}, loss {args.loss}%", flush=True)
                deadline = began + args.seconds + 45
                while not (state / "result.json").is_file():
                    if time.monotonic() > deadline or not inspect(load)["State"]["Running"]:
                        raise RuntimeError("Selected HTTPS test failed; no application replay attempted")
                    time.sleep(.2)
                elapsed = time.monotonic() - began
                measured = json.loads((state / "result.json").read_text())
                if "failure" in measured:
                    raise RuntimeError("Synthetic probe failed while " + measured["failure"])
                for name, path, old in zip(("relay", "load"), paths, before):
                    used, peak = counters(path)
                    measured[name + "_cpu_percent_one_cpu"] = (used - old) / 1e6 / elapsed * 100
                    measured[name + "_peak_ram_mib"] = peak / 1024**2
                    settings = inspect(relay if name == "relay" else load)["HostConfig"]
                    assert settings["ReadonlyRootfs"] and settings["LogConfig"]["Type"] == "none" and settings["CapDrop"] == ["ALL"]
                measured["measurement_wall_seconds"] = elapsed
                results.append(measured)
                (state / "start").unlink()
                assert command("docker", "wait", load, timeout=10) == "0"
                print(f"  {measured['received']}/{measured['offered']}; p50 {measured['delay_p50_ms']:.1f} ms; p99 {measured['delay_p99_ms']:.1f} ms", flush=True)
            finally:
                remove("docker", "rm", "-f", load, relay)
                shutil.rmtree(state)
        (output / "results.json").write_text(json.dumps({"metadata": metadata, "results": results}, indent=2) + "\n")
        print(f"Aggregate evidence: {output / 'results.json'}", flush=True)
    finally:
        remove("docker", "rm", "-f", load, relay)
        remove("docker", "network", "rm", network)
        remove("docker", "image", "rm", image)
        shutil.rmtree(cert)


if __name__ == "__main__":
    main()
