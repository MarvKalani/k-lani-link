"""Linux transport acceptance test of the desktop profile over the HTTPS mailbox.

Real relay (disposable certificate), real host and real desktop transport container
with a synthetic backend. OpenCode itself is not started (it refuses hosts with swap);
a network=none probe on the private API socket stands in for it. Not a WAN, proxy,
Windows or model test.
"""
import argparse
import hashlib
import http.client
import json
import os
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
import tomllib

TRANSPORT = "k-lani-link-desktop-transport:local"
TOOLS = "k-lani-link-desktop-opencode:local"
PRIVATE = {"version": 1, "application_logs": False, "request_history": False,
           "diagnostics": False, "remote_images": False}


def backend():
    """Synthetic OpenAI-compatible server on the host's loopback. It never sees the client key."""
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def reply(self, value):
            body = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            if self.path == "/health":
                return self.reply({"link_privacy": PRIVATE})
            self.reply({"object": "list", "data": [{"id": "https-synthetic", "object": "model"}]})

        def do_POST(self):
            self.rfile.read(int(self.headers["Content-Length"]))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            for word in ("HTTPS", "_DESKTOP", "_OK"):
                event = {"choices": [{"index": 0, "delta": {"content": word}}]}
                self.wfile.write(b"data: " + json.dumps(event).encode() + b"\n\n")
                self.wfile.flush()
                time.sleep(0.05)
            self.wfile.write(b"data: [DONE]\n\n")
    ThreadingHTTPServer(("127.0.0.1", 8080), Handler).serve_forever()


def probe():
    """Inside network=none: the API as OpenCode would reach it."""
    if {name for _, name in socket.if_nameindex()} != {"lo"}:
        raise SystemExit("probe must run without network")
    key = tomllib.loads(Path("/run/secrets/client.toml").read_text())["api_key"]
    def request(method, path, body=None):
        deadline = time.monotonic() + 90
        while True:
            connection = http.client.HTTPConnection("localhost", timeout=30)
            try:
                connection.sock = socket.socket(socket.AF_UNIX)
                connection.sock.connect("/run/link-api/api.sock")
                connection.request(method, path, body=body, headers={
                    "Authorization": "Bearer " + key, "Content-Type": "application/json"})
                response = connection.getresponse()
                data = response.read()
                if response.status == 200 or time.monotonic() > deadline:
                    return response.status, data
            except OSError:
                if time.monotonic() > deadline:
                    raise
            finally:
                connection.close()
            time.sleep(0.5)
    status, models = request("GET", "/v1/models")
    if status != 200:
        _, health = request("GET", "/health")
        raise SystemExit(f"model list failed: {status} {models[:300]!r}; gateway health {health[:400]!r}")
    assert json.loads(models)["data"][0]["id"] == "https-synthetic", "model list failed"
    status, stream = request("POST", "/v1/chat/completions", json.dumps(
        {"model": "https-synthetic", "stream": True, "messages": [{"role": "user", "content": "hi"}]}))
    text = "".join(json.loads(line[6:])["choices"][0]["delta"].get("content", "")
                   for line in stream.decode().splitlines() if line.startswith("data: {"))
    assert status == 200 and text == "HTTPS_DESKTOP_OK" and stream.endswith(b"data: [DONE]\n\n"), "stream failed"
    print(json.dumps({"models": True, "streamed": text}))


def commands(env):
    def command(args, **kwargs):
        result = subprocess.run(args, env=env, capture_output=True, text=True, **kwargs)
        if result.returncode:
            raise RuntimeError("https smoke command failed: " + " ".join(args[:6]) + "\n" + result.stderr[-2000:])
        return result.stdout
    return command


def relay_fixture(root, command):
    """A relay folder with one token and a disposable certificate for the name `relay`."""
    relay = root / "relay"
    relay.mkdir(mode=0o700)
    token = secrets.token_urlsafe(32)
    for path in (relay / "relay-token", root / "token"):
        path.write_text(token)
        path.chmod(0o600)
    # The client trusts only this certificate.
    command(["openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256",
             "-nodes", "-days", "1", "-subj", "/CN=relay", "-addext", "subjectAltName=DNS:relay",
             # A server certificate: TLS clients reject a CA certificate used as the end entity.
             "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "extendedKeyUsage=serverAuth",
             "-keyout", str(relay / "server.key"), "-out", str(relay / "server.crt")])
    (relay / "server.key").chmod(0o600)
    (relay / "relay.toml").write_text('listen = "0.0.0.0:8789"\ntls_cert = "server.crt"\n'
                                      'tls_key = "server.key"\n\n[[tokens]]\ntoken_file = "relay-token"\n')
    return relay, token


def services(relay, host_folder):
    """Relay, synthetic backend and a host serving `host_folder`, beside the desktop profile."""
    return f"""services:
  relay:
    image: {TRANSPORT}
    command: [https-relay, --config, /relay/relay.toml]
    networks: [transport]
    read_only: true
    cap_drop: [ALL]
    logging: {{driver: none}}
    volumes: ['{relay}:/relay:ro']
  backend:
    image: python:3.12-slim
    user: '1000:1000'
    entrypoint: [python, -B, /test/https_smoke.py, --backend]
    networks: [transport]
    read_only: true
    cap_drop: [ALL]
    logging: {{driver: none}}
    volumes: ['{Path(__file__).resolve()}:/test/https_smoke.py:ro']
  link:
    logging: {{driver: local}}
  host:
    image: {TRANSPORT}
    command: [host, --config, /run/link-host/host.toml]
    network_mode: service:backend
    depends_on: [backend, relay]
    read_only: true
    cap_drop: [ALL]
    logging: {{driver: local}}
    volumes: ['{host_folder}:/run/link-host']
"""


def run_by_code():
    """Pairing by code as Manage.ps1 -Action Pair does it: no client folder ever leaves the host."""
    root_dir = Path(__file__).resolve().parents[2]
    if os.getuid() != 1000:
        raise SystemExit("Run as UID 1000 on a Linux Docker test host.")
    project = "link-desktop-code-" + secrets.token_hex(5)
    with tempfile.TemporaryDirectory(prefix=project + "-") as temporary:
        root = Path(temporary)
        (root / "work").mkdir()
        # The launcher's placeholders: preparation reads the joined volume, never this folder.
        env = dict(os.environ, LINK_WORKSPACE=str(root / "work"), LINK_CLIENT_DIR="/unused",
                   LINK_CLIENT_FILE="client.toml", LINK_PROXY_URL="")
        command = commands(env)
        relay, token = relay_fixture(root, command)
        override = root / "test.yaml"
        override.write_text(services(relay, root / "offered/host"))
        compose = ["docker", "compose", "-p", project, "-f", str(root_dir / "deploy/desktop/compose.yaml"),
                   "-f", str(override)]
        run = compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never"]
        def present(service, flag, path):
            result = subprocess.run(run + ["--entrypoint", "test", service, flag, path], env=env,
                                    capture_output=True, text=True)
            assert result.returncode in (0, 1), result.stderr[-1000:]
            return result.returncode == 0
        offer = None
        try:
            command(compose + ["up", "-d", "--pull", "never", "relay"], timeout=60)
            assert not present("link", "-e", "/var/lib/link/https"), "fresh profile already paired"
            assert not present("join", "-f", "/var/lib/link/client/client.toml"), "stale joined pairing"
            # The home side, as the host container's `offer` service runs it.
            offer = subprocess.Popen(
                ["docker", "run", "--rm", "--network", project + "_transport", "--log-driver", "none",
                 "-v", str(root) + ":/fixture", TRANSPORT, "offer-https", "--dir", "/fixture/offered",
                 "--relay-url", "https://relay:8789", "--admission-token-file", "/fixture/token",
                 "--ca-cert-file", "/fixture/relay/server.crt"],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            first = offer.stdout.readline()
            assert first.startswith("Pairing code: "), first + offer.stderr.read()[-1000:]
            code = first.split(": ", 1)[1].strip()
            # As PowerShell pipes it: with a Windows line end, on standard input only.
            joined = command(run + ["-v", f"{relay / 'server.crt'}:/run/pair-ca/ca.pem:ro", "join",
                                    "--relay-url", "https://relay:8789", "--ca-cert-file",
                                    "/run/pair-ca/ca.pem"], input=code + "\r\n", timeout=120)
            assert "Paired." in joined and token not in joined, "join failed"
            assert offer.wait(timeout=60) == 0, offer.stderr.read()[-1000:]
            assert present("join", "-f", "/var/lib/link/client/client.toml"), "nothing was joined"
            prepared = command(run + ["prepare-joined"])
            assert "HTTPS pairing state: IMPORTED" in prepared and token not in prepared, "import failed"
            command(["docker", "volume", "rm", project + "_joined"])
            assert present("link", "-e", "/var/lib/link/https"), "pairing not live"
            assert not present("join", "-f", "/var/lib/link/client/client.toml"), "received copy kept"
            command(run + ["link", "check", "--config", "/run/secrets/client.toml"])
            command(compose + ["up", "-d", "--pull", "never", "relay", "backend", "host", "link"], timeout=120)
            evidence = command(run + ["--entrypoint", "python", "-v",
                                      str(Path(__file__).resolve()) + ":/test/https_smoke.py:ro",
                                      "opencode", "-B", "/test/https_smoke.py", "--probe"], timeout=150)
            print(evidence.strip())
            print("PASS: code offered by the host, joined into a private volume, imported, received "
                  "copy removed, relay session and streamed answer through the desktop transport.")
        except BaseException:
            status = subprocess.run(compose + ["ps", "-a", "--format", "{{.Service}} {{.State}} {{.Status}}"],
                                    env=env, capture_output=True, text=True)
            print(status.stdout)
            for service in ("link", "host"):
                logs = subprocess.run(compose + ["logs", "--no-color", "--tail", "20", service],
                                      env=env, capture_output=True, text=True)
                print(f"--- {service}\n{logs.stdout[-2000:]}{logs.stderr[-500:]}")
            raise
        finally:
            if offer and offer.poll() is None:
                offer.kill()
            # Only the unique disposable test project is removed.
            subprocess.run(compose + ["down", "--volumes", "--remove-orphans"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60, check=False)
            # Recreated empty by the last check; not every Compose removes a `run`-only volume.
            subprocess.run(["docker", "volume", "rm", project + "_joined"], stdout=subprocess.DEVNULL,
                           stderr=subprocess.DEVNULL, timeout=30, check=False)


def run():
    root_dir = Path(__file__).resolve().parents[2]
    if os.getuid() != 1000:
        raise SystemExit("Run as UID 1000 on a Linux Docker test host.")
    project = "link-desktop-https-" + secrets.token_hex(5)
    with tempfile.TemporaryDirectory(prefix=project + "-") as temporary:
        root = Path(temporary)
        work = root / "work"
        work.mkdir()
        env = dict(os.environ, LINK_WORKSPACE=str(work), LINK_CLIENT_DIR=str(root / "pair/client"),
                   LINK_CLIENT_FILE="client.toml")
        command = commands(env)
        relay, token = relay_fixture(root, command)
        def issue(name):
            command(["docker", "run", "--rm", "--network", "none", "--log-driver", "none",
                     "-v", str(root) + ":/fixture", TRANSPORT, "init-https", "--dir", "/fixture/" + name,
                     "--relay-url", "https://relay:8789", "--admission-token-file", "/fixture/token",
                     "--ca-cert-file", "/fixture/relay/server.crt"])
        issue("pair")
        issue("other")
        source_chain = (root / "pair/client/state/client.chain").read_bytes()
        override = root / "test.yaml"
        override.write_text(services(relay, root / "pair/host"))
        compose = ["docker", "compose", "-p", project, "-f", str(root_dir / "deploy/desktop/compose.yaml"),
                   "-f", str(override)]
        def chain():
            # Read the live state from the private volume as UID 1000, without printing it.
            script = ("import hashlib,pathlib;print(hashlib.sha256(pathlib.Path("
                      "'/s/https/client.chain').read_bytes()).hexdigest())")
            return command(["docker", "run", "--rm", "--network", "none", "--log-driver", "none",
                            "-v", project + "_pairing_state:/s:ro", "--entrypoint", "python", TOOLS,
                            "-B", "-c", script]).strip()
        try:
            prepared = command(compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never", "prepare"])
            assert "HTTPS pairing state: IMPORTED" in prepared and token not in prepared, "first import failed"
            assert chain() == hashlib.sha256(source_chain).hexdigest()
            command(compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never", "link", "check",
                               "--config", "/run/secrets/client.toml"])
            command(compose + ["up", "-d", "--pull", "never", "relay", "backend", "host", "link"], timeout=120)
            evidence = command(compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never",
                                          "--entrypoint", "python", "-v",
                                          str(Path(__file__).resolve()) + ":/test/https_smoke.py:ro",
                                          "opencode", "-B", "/test/https_smoke.py", "--probe"], timeout=150)
            print(evidence.strip())
            command(compose + ["stop", "link"])
            # The live file also records activity, so compare only once the client stopped.
            advanced = chain()
            assert advanced != hashlib.sha256(source_chain).hexdigest(), "pairing state did not advance"
            # A later start keeps the advanced volume state, even with the stale source folder.
            kept = command(compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never", "prepare"])
            assert "HTTPS pairing state: KEPT" in kept and chain() == advanced, "state was not kept"
            command(compose + ["up", "-d", "--pull", "never", "link"], timeout=60)
            command(compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never", "--entrypoint", "python",
                               "-v", str(Path(__file__).resolve()) + ":/test/https_smoke.py:ro",
                               "opencode", "-B", "/test/https_smoke.py", "--probe"], timeout=150)
            command(compose + ["stop", "link"])
            # A different pair is refused instead of replacing the live one.
            env["LINK_CLIENT_DIR"] = str(root / "other/client")
            refused = subprocess.run(compose + ["run", "--rm", "-T", "--no-deps", "--pull", "never", "prepare"],
                                     env=env, capture_output=True, text=True)
            assert refused.returncode and "different HTTPS pair" in refused.stderr, "different pair accepted"
            assert token not in refused.stdout + refused.stderr
            print("PASS: HTTPS import, relay session through the desktop transport, streamed answer, "
                  "advanced state kept on restart, different pair refused.")
        except BaseException:
            # Test-only diagnostics: synthetic data, no pairing material is logged by these services.
            status = subprocess.run(compose + ["ps", "-a", "--format", "{{.Service}} {{.State}} {{.Status}}"],
                                    env=env, capture_output=True, text=True)
            print(status.stdout)
            for service in ("link", "host"):
                logs = subprocess.run(compose + ["logs", "--no-color", "--tail", "20", service],
                                      env=env, capture_output=True, text=True)
                print(f"--- {service}\n{logs.stdout[-2000:]}{logs.stderr[-500:]}")
            raise
        finally:
            # Only the unique disposable test project is removed.
            env["LINK_CLIENT_DIR"] = str(root / "pair/client")
            subprocess.run(compose + ["down", "--volumes", "--remove-orphans"], env=env,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60, check=False)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", action="store_true")
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--by-code", action="store_true", help="pair by code instead of a copied folder")
    args = parser.parse_args()
    if args.backend:
        backend()
    elif args.probe:
        probe()
    elif args.by_code:
        run_by_code()
    else:
        run()
