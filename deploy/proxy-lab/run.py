"""Reproduce corporate egress/TLS inspection against the actual k-lani-link CLI.

Only uniquely labelled containers/networks are changed. Runtime networks are internal;
there are no host ports, host-network containers, Docker socket mounts or model mounts.
"""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import signal
import subprocess
import sys
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
BINARY = ROOT / "target/x86_64-unknown-linux-musl/release/k-lani-link"
MITM = "mitmproxy/mitmproxy@sha256:2711066c855659b5af8b1dbb84494ed3eff69b42ece439c60b4fe09a2db829d4"
RELAY = "https://relay.corp.test"
CASES = (
    "inspected_https", "pairing_by_code", "domain_denied", "missing_inspection_ca", "wrong_origin_name",
    "expired_origin_certificate", "basic_proxy_auth", "http1_only", "post_only",
    "json_only", "buffered", "lost_write_ack", "proxy_restart", "revoke_domain",
    "revoke_token", "tampered_record", "blocked_quic",
)


def command(*args, timeout=90, check=True):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True,
                            timeout=timeout, cwd=ROOT)
    if check and result.returncode:
        # Commands can return a pairing code; never include their output in exceptions.
        raise RuntimeError(f"{Path(str(args[0])).name} {args[1] if len(args) > 1 else ''} failed ({result.returncode})")
    return result


def write(path, text):
    path = Path(path)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(text)
    temporary.chmod(0o600)
    temporary.replace(path)


def json_write(path, value):
    write(path, json.dumps(value, indent=2) + "\n")


def read_json(path, default=None):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError):
        return default


def configure(path, section, values):
    """Edit only named scalar keys in CLI-generated TOML; validate the result afterwards."""
    lines = Path(path).read_text().splitlines()
    begin = 0 if section is None else lines.index("[" + section + "]") + 1
    end = next((n for n in range(begin, len(lines)) if lines[n].startswith("[")), len(lines))
    remaining = dict(values)
    output = lines[:begin]
    for line in lines[begin:end]:
        key = line.split("=", 1)[0].strip()
        if key in remaining:
            output.append(key + " = " + json.dumps(remaining.pop(key)))
        else:
            output.append(line)
    output.extend(key + " = " + json.dumps(value) for key, value in remaining.items())
    text = "\n".join(output + lines[end:]) + "\n"
    tomllib.loads(text)
    write(path, text)


def certificates(directory):
    directory.mkdir(mode=0o700)
    for name in ("origin", "inspection"):
        command("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256",
                "-days", "2", "-subj", f"/CN=k-lani proxy lab {name} CA",
                "-addext", "basicConstraints=critical,CA:TRUE",
                "-addext", "keyUsage=critical,keyCertSign,cRLSign",
                "-keyout", directory / f"{name}.key", "-out", directory / f"{name}.pem")
    for name, hostname, days in (("valid", "relay.corp.test", "2"),
                                  ("wrong", "different.corp.test", "2"),
                                  ("expired", "relay.corp.test", "-1")):
        command("openssl", "req", "-newkey", "rsa:2048", "-nodes",
                "-subj", f"/CN={hostname}", "-keyout", directory / f"{name}.key",
                "-out", directory / f"{name}.csr")
        write(directory / "extensions.conf", f"subjectAltName=DNS:{hostname}\n"
              "basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
              "extendedKeyUsage=serverAuth\n")
        if name != "expired":
            command("openssl", "x509", "-req", "-in", directory / f"{name}.csr",
                    "-CA", directory / "origin.pem", "-CAkey", directory / "origin.key",
                    "-CAcreateserial", "-days", days, "-sha256", "-extfile", directory / "extensions.conf",
                    "-out", directory / f"{name}.pem")
        else:
            (directory / "issued").mkdir(mode=0o700)
            write(directory / "index.txt", "")
            write(directory / "serial.txt", "01\n")
            write(directory / "ca.conf", "[ca]\ndefault_ca = lab\n[lab]\n"
                  f"database = {directory}/index.txt\nserial = {directory}/serial.txt\n"
                  f"new_certs_dir = {directory}/issued\ncertificate = {directory}/origin.pem\n"
                  f"private_key = {directory}/origin.key\ndefault_md = sha256\npolicy = names\n"
                  "[names]\ncommonName = supplied\n[server]\n" + (directory / "extensions.conf").read_text())
            now = dt.datetime.now(dt.UTC)
            command("openssl", "ca", "-batch", "-notext", "-config", directory / "ca.conf",
                    "-extensions", "server", "-in", directory / "expired.csr",
                    "-startdate", (now - dt.timedelta(days=2)).strftime("%Y%m%d%H%M%SZ"),
                    "-enddate", (now - dt.timedelta(days=1)).strftime("%Y%m%d%H%M%SZ"),
                    "-out", directory / "expired.pem")
    for path in directory.iterdir():
        path.chmod(0o700 if path.is_dir() else 0o600)


class Lab:
    def __init__(self, output, image):
        self.output, self.image = output, image
        self.private = output / "private"
        self.private.mkdir(mode=0o700)
        self.cert = self.private / "certificates"
        certificates(self.cert)
        self.id = "klproxy-" + uuid.uuid4().hex[:10]
        self.label = "io.k-lani.proxy-lab.run=" + self.id
        self.containers = []
        self.networks = []

    def docker(self, *args, **kwargs):
        return command("docker", *args, **kwargs)

    def network(self, kind):
        name = self.id + "-" + kind
        self.docker("network", "create", "--internal", "--label", self.label, name)
        self.networks.append(name)
        return name

    def container(self, name, network, image=None, extra=(), args=(), uid=True):
        name = self.id + "-" + name
        options = ["run", "-d", "--name", name, "--label", self.label,
                   "--network", network, "--cap-drop", "ALL", "--read-only",
                   "--security-opt", "no-new-privileges:true", "--pids-limit", "128",
                   "--memory", "384m", "--memory-swap", "384m", "--cpus", "1",
                   "--ulimit", "core=0", "--tmpfs", "/tmp:rw,nosuid,nodev,size=32m",
                   "--log-driver", "local", "--log-opt", "max-size=1m", "--log-opt", "max-file=2",
                   "--sysctl", "net.ipv4.ip_unprivileged_port_start=0"]
        if uid:
            options += ["--user", f"{os.getuid()}:{os.getgid()}"]
        self.containers.append(name)
        self.docker(*options, *extra, image or self.image, *args)
        return name

    def inspect(self, name):
        return json.loads(self.docker("inspect", name).stdout)[0]

    def address(self, name, network):
        return self.inspect(name)["NetworkSettings"]["Networks"][network]["IPAddress"]

    def cleanup_case(self):
        for name in reversed(self.containers):
            self.docker("rm", "-f", name, check=False)
        self.containers.clear()
        for name in reversed(self.networks):
            self.docker("network", "rm", name, check=False)
        self.networks.clear()

    def endpoint(self, action, *args, timeout=60):
        env = ("-e", "LAB_CLIENT_CONFIG=" + self.active_config) if self.active_config else ()
        result = self.docker("exec", *env, self.client, "python3", "/lab/endpoint.py", action,
                             *args, timeout=timeout)
        return json.loads(result.stdout)

    def pair_by_code(self):
        """Exercise the actual offer/join protocol through the inspecting proxy."""
        shutil.copyfile(self.case / "relay/client-token", self.case / "pair/host/code-client-token")
        with (self.case / "pairing-output.txt").open("w") as output:
            process = subprocess.Popen(["docker", "exec", self.host, "k-lani-link", "offer-https",
                "--dir", "/state/code-offer", "--relay-url", RELAY,
                "--admission-token-file", "/state/relay-token",
                "--client-admission-token-file", "/state/code-client-token", "--ca-cert-file", "/state/ca.pem"],
                stdout=output, stderr=subprocess.DEVNULL)
        try:
            until = time.monotonic() + 15
            code = None
            while time.monotonic() < until:
                match = re.search(r"Pairing code: ([0-9-]+)", (self.case / "pairing-output.txt").read_text())
                if match:
                    code = match.group(1)
                    break
                time.sleep(0.1)
            if code is None:
                raise RuntimeError("offer did not produce a pairing code")
            joined = subprocess.run(["docker", "exec", "-i", self.client, "k-lani-link", "join-https",
                "--dir", "/state/code-joined", "--relay-url", RELAY,
                "--proxy-url", "http://proxy.corp.test:8080", "--ca-cert-file", "/state/ca.pem"],
                input=code + "\n", capture_output=True, text=True, timeout=25)
            if joined.returncode or process.wait(timeout=15):
                raise RuntimeError("pairing by code failed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
        configure(self.client_state / "code-joined/client.toml", None,
                  {"listen": "127.0.0.1:8788", "request_timeout_seconds": 45, "resume_seconds": 20})
        for path in (self.client_state / "code-joined/client.toml", self.case / "pair/host/code-offer/host/host.toml"):
            configure(path, "https", {"poll_wait_seconds": 1})
        self.docker("exec", "-d", self.host, "k-lani-link", "host", "--config", "/state/code-offer/host/host.toml")
        self.docker("exec", "-d", self.client, "k-lani-link", "client", "--config", "/state/code-joined/client.toml")
        self.active_config = "/state/code-joined/client.toml"
        return self.endpoint("wait", "--timeout", "20").get("connected", False)

    def policy(self, **changes):
        value = read_json(self.case / "control/policy.json", {})
        value.update(changes)
        value["generation"] = value.get("generation", 0) + 1
        json_write(self.case / "control/policy.json", value)

    def events(self):
        path = self.case / "audit/events.jsonl"
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text().splitlines() if line]

    def wait_proxy(self):
        until = time.monotonic() + 15
        while time.monotonic() < until:
            if any(e["event"] == "ready" for e in self.events()):
                return
            if not self.inspect(self.proxy)["State"]["Running"]:
                raise RuntimeError("inspection proxy did not start")
            time.sleep(0.1)
        raise RuntimeError("inspection proxy startup deadline")

    def start(self, name):
        self.active_config = None
        self.case = self.private / name
        self.case.mkdir(mode=0o700)
        for path in ("relay", "control", "audit", "mitm"):
            (self.case / path).mkdir(mode=0o700)
        origin = "wrong" if name == "wrong_origin_name" else "expired" if name == "expired_origin_certificate" else "valid"
        for source, target in ((f"{origin}.pem", "server.pem"), (f"{origin}.key", "server.key"),
                               ("origin.pem", "origin.pem")):
            shutil.copyfile(self.cert / source, self.case / "relay" / target)
        for kind in ("host", "client"):
            write(self.case / "relay" / (kind + "-token"), secrets.token_urlsafe(32))
        write(self.case / "mitm/mitmproxy-ca.pem", (self.cert / "inspection.key").read_text()
              + (self.cert / "inspection.pem").read_text())
        shutil.copyfile(self.cert / "inspection.pem", self.case / "mitm/mitmproxy-ca-cert.pem")
        command(BINARY, "init-https", "--dir", self.case / "pair", "--relay-url", RELAY,
                "--admission-token-file", self.case / "relay/host-token",
                "--client-admission-token-file", self.case / "relay/client-token",
                "--ca-cert-file", self.cert / "origin.pem")
        self.client_state = self.case / "pair/client"
        for side in ("host", "client"):
            configure(self.case / f"pair/{side}/{side}.toml", None,
                      {"request_timeout_seconds": 45, "resume_seconds": 20})
            configure(self.case / f"pair/{side}/{side}.toml", "https", {"poll_wait_seconds": 1})
        quic = name == "blocked_quic"
        if quic:
            configure(self.client_state / "client.toml", "https", {"http3": True})
        else:
            proxy_url = "http://proxy.corp.test:8080"
            if name == "basic_proxy_auth":
                credentials = "lab:" + secrets.token_hex(16)
                write(self.case / "control/proxy-auth", credentials)
                proxy_url = "http://" + credentials + "@proxy.corp.test:8080"
            configure(self.client_state / "client.toml", "https", {"proxy_url": proxy_url})
            if name != "missing_inspection_ca":
                shutil.copyfile(self.cert / "inspection.pem", self.client_state / "ca.pem")
        write(self.case / "relay/relay.toml", 'listen = "0.0.0.0:443"\n'
              'tls_cert = "server.pem"\ntls_key = "server.key"\nhttp3 = true\n'
              '[[tokens]]\ntoken_file = "host-token"\nspace = "lab"\n'
              '[[tokens]]\ntoken_file = "client-token"\nspace = "lab"\n')
        self.policy(allow=name != "domain_denied", hosts=["relay.corp.test"],
                    mode=name if name in {"post_only", "json_only", "buffered"} else "allow",
                    require_auth=name == "basic_proxy_auth")
        self.corp, self.transit, self.backend_net = [self.network(n) for n in ("corp", "transit", "backend")]
        self.relay = self.container("relay", self.transit,
            extra=("--network-alias", "relay.corp.test", "-v", f"{self.case / 'relay'}:/config:ro",
                   "--entrypoint", "/usr/local/bin/k-lani-link"),
            args=("https-relay", "--config", "/config/relay.toml"))
        self.docker("network", "connect", "--alias", "relay.corp.test", self.backend_net, self.relay)
        self.proxy = self.container("proxy", self.corp, image=MITM,
            extra=("--network-alias", "proxy.corp.test", "-v", f"{HERE}:/lab:ro",
                   "-v", f"{self.case / 'mitm'}:/mitm:rw", "-v", f"{self.case / 'control'}:/control:ro",
                   "-v", f"{self.case / 'relay/origin.pem'}:/origin.pem:ro",
                   "-v", f"{self.case / 'audit'}:/audit:rw", "--entrypoint", "mitmdump"),
            args=("-q", "--listen-host", "0.0.0.0", "--listen-port", "8080", "-s", "/lab/proxy.py",
                  "--set", "confdir=/mitm", "--set", "ssl_verify_upstream_trusted_ca=/origin.pem",
                  "--set", "ssl_insecure=false", "--set", "connection_strategy=lazy",
                  "--set", "http2=" + ("false" if name == "http1_only" else "true")))
        self.docker("network", "connect", self.transit, self.proxy)
        self.wait_proxy()
        self.host = self.container("host", self.backend_net,
            extra=("-v", f"{self.case / 'pair/host'}:/state:rw"), args=("host",))
        # Start a harmless keeper first, so QUIC can be blocked before the actual client starts.
        self.client = self.container("client", self.transit if quic else self.corp,
            extra=("-v", f"{self.client_state}:/state:rw", "--entrypoint", "python3"),
            args=("-c", "import time; time.sleep(3600)"))
        if quic:
            self.docker("run", "--rm", "--label", self.label, "--network", "container:" + self.client,
                        "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "--security-opt", "no-new-privileges:true",
                        "--read-only", "--entrypoint", "iptables", self.image,
                        "-A", "OUTPUT", "-p", "udp", "--dport", "443", "-j", "DROP")
        self.docker("exec", "-d", self.client, "k-lani-link", "client", "--config", "/state/client.toml")

    def audit(self, name):
        events = self.events()
        counts = Counter(e["event"] for e in events)
        requests = [e for e in events if e["event"] == "request"]
        return {
            "events": dict(counts),
            "response_statuses": dict(Counter(str(e["status"]) for e in events if e["event"] == "response")),
            "http_versions": sorted({e["http_version"] for e in requests}),
            "methods": sorted({e["method"] for e in requests}),
            "json_writes": sum(e["method"] == "POST" and e["content_type"] == "application/json" for e in requests),
            "plaintext_marker_found": any(e.get("plaintext_marker") for e in events),
            "client_tls_inspected": counts["client_tls"] > 0 and any(e.get("tls") for e in requests),
            "origin_tls_verified": counts["server_tls"] > 0,
            "backend": read_json(self.case / "pair/host/backend.json", {"calls": 0, "duplicates": 0}),
        }

    def run_case(self, name):
        began = time.monotonic()
        details = {"case": name, "passed": False}
        try:
            self.start(name)
            negative = name in {"domain_denied", "missing_inspection_ca", "wrong_origin_name", "expired_origin_certificate"}
            state = self.endpoint("wait", "--timeout", "5" if negative else "20")
            details["connection"] = state
            if negative:
                evidence = self.audit(name)
                checks = {"valid_health_response": isinstance(state.get("connected"), bool),
                          "connection_refused": state.get("connected") is False,
                          "backend_not_called": evidence["backend"]["calls"] == 0}
                event = {"domain_denied": "connect_denied", "missing_inspection_ca": "client_tls_failed",
                         "wrong_origin_name": "server_tls_failed", "expired_origin_certificate": "server_tls_failed"}[name]
                checks["expected_rejection_observed"] = evidence["events"].get(event, 0) > 0
                details.update(checks=checks, evidence=evidence, passed=all(checks.values()))
                return details
            if not state.get("connected"):
                details["error"] = "authenticated connection did not become ready"
                details["evidence"] = self.audit(name)
                return details
            checks = {}
            if name == "inspected_https":
                isolation = self.endpoint("connectivity", "--relay-ip", self.address(self.relay, self.transit),
                                          "--host-ip", self.address(self.host, self.backend_net))
                details["network_probes"] = isolation
                checks["only_proxy_reachable"] = (isolation["proxy_reachable"]
                    and not isolation["relay_direct_reachable"] and not isolation["model_direct_reachable"]
                    and not isolation["default_ipv4_route"])
            if name == "pairing_by_code":
                checks["pairing_code_round_trip"] = self.pair_by_code()
            if name == "lost_write_ack":
                self.policy(mode="drop_once")
            if name == "tampered_record":
                self.policy(mode="tamper_once")
            if name == "proxy_restart":
                process = subprocess.Popen(["docker", "exec", self.client, "python3", "/lab/endpoint.py",
                    "probe", "--tokens", "160", "--delay", "0.04", "--timeout", "45"],
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                deadline = time.monotonic() + 12
                while time.monotonic() < deadline:
                    if read_json(self.client_state / "progress.json", {}).get("tokens", 0) >= 3:
                        break
                    time.sleep(0.1)
                else:
                    process.kill()
                    process.communicate()
                    raise RuntimeError("stream did not start before proxy restart")
                self.docker("restart", "--time", "1", self.proxy)
                stdout, _ = process.communicate(timeout=55)
                result = json.loads(stdout)
            else:
                result = self.endpoint("probe", "--timeout", "15", timeout=45)
            details["probe"] = result
            checks["stream_complete"] = result["complete"]
            if name == "tampered_record":
                del checks["stream_complete"]
                checks["corruption_rejected"] = not result["complete"]
                details["after_tampering"] = self.endpoint("health")
                checks["authentication_failure_reported"] = details["after_tampering"].get("pairing_rejected") is True
            if name in {"revoke_domain", "revoke_token"}:
                before = read_json(self.case / "pair/host/backend.json")["calls"]
                if name == "revoke_domain":
                    self.policy(allow=False)
                    time.sleep(1)
                    rejected = self.endpoint("probe", "--timeout", "7", timeout=20)
                else:
                    revoked_at = time.monotonic()
                    write(self.case / "relay/client-token", secrets.token_urlsafe(32))
                    self.docker("restart", "--time", "1", self.relay)
                    # Existing HTTP/2 streams may fail before a fresh relay connection
                    # can report 401. Allow the application's bounded recovery window.
                    # Submit work DURING that window and prove it never reaches the model.
                    with ThreadPoolExecutor(max_workers=1) as pool:
                        pending = pool.submit(self.endpoint, "probe", "--timeout", "35", timeout=40)
                        denied = self.endpoint("wait-denied", "--timeout", "40", timeout=45)
                        observed_after = time.monotonic() - revoked_at
                        rejected = pending.result()
                    details["denied_health"] = denied
                    details["revocation_observed_after_seconds"] = round(observed_after, 3)
                    checks["client_reports_access_denied"] = denied.get("access_denied") is True
                details["after_revocation"] = rejected
                after = read_json(self.case / "pair/host/backend.json")["calls"]
                checks["revocation_blocks_work"] = not rejected["complete"] and before == after
            evidence = self.audit(name)
            details["evidence"] = evidence
            checks["no_duplicate_model_execution"] = evidence["backend"]["duplicates"] == 0
            if name != "tampered_record":
                checks["one_model_execution"] = evidence["backend"]["calls"] == 1
            if name != "blocked_quic":
                checks["no_plaintext_at_inspector"] = not evidence["plaintext_marker_found"]
                checks["tls_actually_inspected"] = evidence["client_tls_inspected"]
                checks["origin_tls_verified"] = evidence["origin_tls_verified"]
            if name == "http1_only":
                checks["negotiated_http1"] = evidence["http_versions"] == ["HTTP/1.1"]
            if name in {"post_only", "json_only"}:
                checks["post_fallback_observed"] = "PUT" in evidence["methods"] and "POST" in evidence["methods"]
            if name == "json_only":
                checks["json_fallback_observed"] = evidence["json_writes"] > 0
            if name == "basic_proxy_auth":
                checks["proxy_credentials_accepted"] = evidence["events"].get("proxy_auth_accepted", 0) > 0
            if name == "lost_write_ack":
                checks["fault_triggered"] = evidence["events"].get("dropped_write_ack", 0) == 1
            if name == "proxy_restart":
                checks["proxy_restart_observed"] = evidence["events"].get("ready", 0) == 2
            if name == "revoke_token":
                checks["relay_denial_observed"] = evidence["response_statuses"].get("401", 0) > 0
            if name == "revoke_domain":
                checks["policy_denial_observed"] = (evidence["response_statuses"].get("403", 0) > 0
                                                    or evidence["events"].get("connect_denied", 0) > 0)
            if name == "tampered_record":
                checks["fault_triggered"] = evidence["events"].get("tampered_record", 0) == 1
            if name == "blocked_quic":
                counters = self.docker("run", "--rm", "--label", self.label, "--network", "container:" + self.client,
                    "--cap-drop", "ALL", "--cap-add", "NET_ADMIN", "--read-only", "--entrypoint", "iptables",
                    self.image, "-L", "OUTPUT", "-v", "-n", "-x").stdout
                matched = re.search(r"^\s*(\d+)\s+\d+\s+DROP\s+udp\b", counters, re.M)
                details["udp_packets_rejected"] = int(matched.group(1)) if matched else 0
                checks["udp_attempt_rejected"] = details["udp_packets_rejected"] > 0
            details.update(checks=checks, passed=all(checks.values()))
            return details
        except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
            details["error"] = str(error) if isinstance(error, RuntimeError) else type(error).__name__
            if hasattr(self, "case"):
                details["evidence"] = self.audit(name)
            return details
        finally:
            details["elapsed_seconds"] = round(time.monotonic() - began, 2)
            # Retain only the explicitly sanitised audit stream outside the secret area.
            if hasattr(self, "case") and (self.case / "audit/events.jsonl").exists():
                shutil.copyfile(self.case / "audit/events.jsonl", self.output / (name + "-audit.jsonl"))
            self.cleanup_case()


def build(output, skip):
    if not skip:
        with (output / "build.log").open("w") as log:
            result = subprocess.run(["cargo", "build", "--release", "--locked", "--target",
                "x86_64-unknown-linux-musl", "--bin", "k-lani-link", "--jobs", "2"],
                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, timeout=1800)
        if result.returncode:
            raise RuntimeError("Cargo build failed; see build.log")
    if not BINARY.is_file():
        raise RuntimeError("Build the Linux x64 musl binary first")
    context = output / "image"
    context.mkdir(mode=0o700)
    shutil.copyfile(BINARY, context / "k-lani-link")
    (context / "k-lani-link").chmod(0o755)
    for name in ("Dockerfile", "endpoint.py"):
        shutil.copyfile(HERE / name, context / name)
    image = "k-lani-link-proxy-lab:" + hashlib.sha256(
        (context / "k-lani-link").read_bytes() + (context / "endpoint.py").read_bytes()
        + (context / "Dockerfile").read_bytes()).hexdigest()[:16]
    with (output / "docker-build.log").open("w") as log:
        result = subprocess.run(["docker", "build", "-t", image, str(context)],
                                stdout=log, stderr=subprocess.STDOUT, timeout=600)
    if result.returncode:
        raise RuntimeError("Docker build failed; see docker-build.log")
    if command("docker", "image", "inspect", MITM, check=False).returncode:
        command("docker", "pull", MITM, timeout=300)
    shutil.rmtree(context)
    return image


def report(output, data):
    json_write(output / "report.json", data)
    rows = ["# Corporate proxy laboratory", "", f"UTC: {data['created_utc']}", "",
            "Actual k-lani-link CLI processes in separate isolated Docker networks. Synthetic model only.", "",
            "| Scenario | Result | Seconds |", "| --- | --- | ---: |"]
    rows += [f"| {r['case']} | {'PASS' if r['passed'] else 'FAIL'} | {r.get('elapsed_seconds', 0)} |" for r in data["cases"]]
    rows += ["", "This is a controlled mitmproxy lab, not a validation of any company's gateway,",
             "URL reputation feed, PAC/NTLM/Kerberos, Windows trust-store handling, or endpoint security software.",
             "The QUIC case is separate from the explicit-proxy cases. Real model and native Windows acceptance remain separate.",
             "Plaintext-marker absence is a regression check, not an independent cryptographic audit.", ""]
    write(output / "REPORT.md", "\n".join(rows))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cases", default=",".join(CASES), help="comma-separated scenario names")
    parser.add_argument("--skip-build", action="store_true", help="use existing musl binary; its hash is recorded")
    parser.add_argument("--keep-private-state", action="store_true", help="retain test credentials for local debugging")
    args = parser.parse_args()
    selected = args.cases.split(",")
    if not selected or any(name not in CASES for name in selected) or len(set(selected)) != len(selected):
        parser.error("Choose unique cases from: " + ", ".join(CASES))
    if platform.system() != "Linux" or platform.machine() not in {"x86_64", "amd64"}:
        parser.error("The runner currently requires Linux x64 with Docker and Python 3.11+")
    os.umask(0o077)
    for executable in ("docker", "openssl"):
        if not shutil.which(executable):
            parser.error(executable + " is required")
    output = ROOT / ".release" / ("proxy-lab-" + dt.datetime.now(dt.UTC).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:6])
    output.mkdir(parents=True, mode=0o700)
    print("Report: " + str(output / "REPORT.md"), flush=True)
    print("Building the test image ...", flush=True)
    image = build(output, args.skip_build)
    data = {"schema": 1, "created_utc": dt.datetime.now(dt.UTC).isoformat(),
            "git_commit": command("git", "rev-parse", "HEAD").stdout.strip(),
            "working_tree_dirty": bool(command("git", "status", "--porcelain").stdout),
            "binary_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
            "lab_sources": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in HERE.iterdir() if p.is_file()},
            "docker_version": command("docker", "version", "--format", "{{.Server.Version}}").stdout.strip(),
            "images": {"endpoints": json.loads(command("docker", "image", "inspect", image).stdout)[0]["Id"],
                       "inspection_proxy": MITM}, "cases": []}
    lab = None
    def interrupted(*unused):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        lab = Lab(output, image)
        data["run_label"] = lab.label
        for name in selected:
            print("Running " + name + " ...", flush=True)
            value = lab.run_case(name)
            data["cases"].append(value)
            report(output, data)
            print(("PASS " if value["passed"] else "FAIL ") + name, flush=True)
    except KeyboardInterrupt:
        data["interrupted"] = True
    except (RuntimeError, OSError, ValueError, subprocess.TimeoutExpired) as error:
        data["runner_error"] = str(error) if isinstance(error, RuntimeError) else type(error).__name__
    finally:
        if lab:
            lab.cleanup_case()
            remaining = {
                "containers": lab.docker("ps", "-aq", "--filter", "label=" + lab.label).stdout.split(),
                "networks": lab.docker("network", "ls", "-q", "--filter", "label=" + lab.label).stdout.split(),
            }
            data["cleanup_complete"] = not any(remaining.values())
        if not args.keep_private_state and (output / "private").exists():
            shutil.rmtree(output / "private")
        data["private_state_retained"] = args.keep_private_state
        data["passed"] = (len(data["cases"]) == len(selected) and all(r["passed"] for r in data["cases"])
                          and data.get("cleanup_complete", False))
        report(output, data)
    print(f"{sum(r['passed'] for r in data['cases'])}/{len(selected)} scenarios passed", flush=True)
    return 0 if data["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
