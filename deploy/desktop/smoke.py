"""Linux container acceptance test with a synthetic API, not a real model or Windows/WAN test."""
import argparse
from collections import Counter
from http.server import BaseHTTPRequestHandler
import http.client
import json
import os
from pathlib import Path
import secrets
import socket
from socketserver import ThreadingMixIn, UnixStreamServer
import subprocess
import tempfile
import time
import tomllib



def backend():
    key = tomllib.loads(Path("/run/secrets/client.toml").read_text())["api_key"]
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            if self.headers.get("Authorization") != "Bearer " + key:
                self.send_error(401)
                return
            body = json.dumps({"data": [{"id": "desktop-synthetic", "object": "model", "meta": {"n_ctx": 262144}}]}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_POST(self):
            if self.path != "/v1/chat/completions" or self.headers.get("Authorization") != "Bearer " + key:
                self.send_error(403)
                return
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            completed = any(m.get("role") == "tool" for m in body["messages"])
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            def send(delta, finish=None):
                data = {"id": "synthetic", "object": "chat.completion.chunk", "created": 1,
                        "model": "desktop-synthetic", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
                self.wfile.write(b"data: " + json.dumps(data).encode() + b"\n\n")
                self.wfile.flush()
                time.sleep(0.05)
            send({"role": "assistant", "reasoning_content": "Synthetic boundary test."})
            if completed:
                send({"content": "DESKTOP_SMOKE_OK"}, "stop")
            else:
                shell = next((t["function"]["name"] for t in body.get("tools", [])
                              if t.get("function", {}).get("name") in {"shell", "bash"}), None)
                if shell is None:
                    raise ValueError("expected shell tool unavailable")
                arguments = json.dumps({"command": "printf 'desktop-smoke-ok\\n' > /work/desktop-smoke.txt",
                                        "description": "Create the synthetic boundary-test file"})
                send({"tool_calls": [{"index": 0, "id": "synthetic-tool", "type": "function",
                                      "function": {"name": shell, "arguments": arguments}}]}, "tool_calls")
            self.wfile.write(b"data: [DONE]\n\n")
    class Server(ThreadingMixIn, UnixStreamServer):
        daemon_threads = True

        def handle_error(self, *_):
            pass
    path = Path("/run/link-api/api.sock")
    path.unlink(missing_ok=True)
    with Server(str(path), Handler) as server:
        path.chmod(0o600)
        server.serve_forever()


def agent():
    env = dict(os.environ)
    env["OPENCODE_PASSWORD"] = Path("/run/secrets/server-password.txt").read_text().strip()
    result = subprocess.run(["opencode", "run", "--server", "http://127.0.0.1:4096", "--format", "json",
                             "--thinking", "--auto", "--model", "link/basalt", "--log-level", "none",
                             "Synthetic test: create desktop-smoke.txt using the shell tool."],
                            env=env, capture_output=True, text=True, timeout=90)
    events = []
    for line in result.stdout.splitlines():
        try:
            events.append(json.loads(line))
        except ValueError:
            pass
    types = Counter(e.get("type") for e in events)
    assert result.returncode == 0 and "DESKTOP_SMOKE_OK" in result.stdout, "synthetic agent failed"
    assert Path("/work/desktop-smoke.txt").read_text() == "desktop-smoke-ok\n", "workspace tool output failed"
    assert types["reasoning"] and types["tool_use"], "reasoning/tool stream missing"
    print(json.dumps({"synthetic_agent": True, "reasoning_events": types["reasoning"],
                      "tool_events": types["tool_use"], "workspace_output_verified": True}))


def run():
    ROOT = Path(__file__).resolve().parents[2]
    if os.getuid() != 1000 or len(Path("/proc/swaps").read_text().splitlines()) != 1:
        raise SystemExit("Run on a Linux Docker test host as UID 1000 with swap disabled.")
    project = "link-desktop-smoke-" + secrets.token_hex(5)
    with tempfile.TemporaryDirectory(prefix=project + "-") as temporary:
        root = Path(temporary)
        work = root / "work"
        work.mkdir()
        # This file must not be visible through the only approved bind mount.
        outside = root / "not-mounted.txt"
        outside.write_text("outside-workspace")
        source = root / "client.toml"
        env = dict(os.environ, LINK_CLIENT_DIR=str(root), LINK_CLIENT_FILE=source.name, LINK_WORKSPACE=str(work))
        compose = ["docker", "compose", "-p", project, "-f", str(ROOT / "deploy/desktop/compose.yaml")]
        def command(args, **kwargs):
            result = subprocess.run(args, env=env, capture_output=True, text=True, **kwargs)
            if result.returncode:
                raise RuntimeError("desktop smoke command failed (no output or credentials retained)")
            return result.stdout
        # Generate genuine random config syntax, then use a synthetic network-free backend.
        command(["docker", "run", "--rm", "--network", "none", "--log-driver", "none",
                 "-v", str(root) + ":/fixture", "k-lani-link-desktop-transport:local",
                 "init", "--dir", "/fixture/pair", "--signaling-backend", "ice-exchange",
                 "--signaling-url", "https://signal.example.invalid", "--stun", "stun:stun.example.invalid:3478"])
        source.write_bytes((root / "pair/client.toml").read_bytes())
        source.chmod(0o600)
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = probe.getsockname()[1]
        override = root / "test.yaml"
        override.write_text("services:\n  link:\n    image: k-lani-link-desktop-opencode:local\n"
                            "    entrypoint: [python, -B, /test/smoke.py, --backend]\n"
                            "    command: []\n    volumes:\n      - '" + str(Path(__file__).resolve()) + ":/test/smoke.py:ro'\n"
                            "  guard:\n    ports: !override ['127.0.0.1:" + str(port) + ":8080']\n")
        try:
            command(compose + ["run", "--rm", "--no-deps", "--pull", "never", "prepare"])
            command(compose + ["run", "--rm", "--no-deps", "--pull", "never", "link", "check", "--config", "/run/secrets/client.toml"])
            test_compose = compose + ["-f", str(override)]
            command(test_compose + ["up", "-d", "--pull", "never", "--wait", "--wait-timeout", "150", "link", "opencode", "guard", "ui"], timeout=180)
            ids = {name: command(test_compose + ["ps", "-q", name]).strip() for name in ("opencode", "link", "guard", "ui")}
            inspection = json.loads(command(["docker", "inspect", ids["opencode"]]))[0]
            binds = [m for m in inspection["Mounts"] if m["Type"] == "bind"]
            assert len(binds) == 1 and binds[0]["Source"] == str(work) and binds[0]["Destination"] == "/work"
            assert inspection["HostConfig"]["NetworkMode"] == "none"
            assert inspection["HostConfig"]["ReadonlyRootfs"] and inspection["HostConfig"]["LogConfig"]["Type"] == "none"
            assert inspection["HostConfig"]["Memory"] == inspection["HostConfig"]["MemorySwap"]
            command(["docker", "exec", ids["opencode"], "python", "-B", "/opt/desktop/verify.py"])
            probe = "import pathlib; assert not pathlib.Path(" + repr(str(outside)) + ").exists()\n"
            command(["docker", "exec", "-i", ids["opencode"], "python", "-B", "-"], input=probe)
            # Observe both TCP and UDP attempts blocked in the UI namespace too.
            network_probe = """import socket
for host in ('1.1.1.1','8.8.8.8','127.0.0.11'):
 for kind,port in ((socket.SOCK_STREAM,443),(socket.SOCK_STREAM,53),(socket.SOCK_DGRAM,53)):
  with socket.socket(socket.AF_INET,kind) as s:
   s.settimeout(.3)
   try:
    s.connect((host,port))
    if kind == socket.SOCK_STREAM: raise AssertionError('unexpected TCP connection')
    s.send(b'\\x12\\x34\\x01\\x00\\x00\\x01\\x00\\x00\\x00\\x00\\x00\\x00\\x07example\\x03com\\x00\\x00\\x01\\x00\\x01'); s.recv(2048)
   except OSError: pass
   else: raise AssertionError('unexpected network reply')
"""
            command(["docker", "run", "--rm", "-i", "--network", "container:" + ids["guard"],
                     "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--log-driver", "none",
                     "--entrypoint", "python", "k-lani-link-desktop-opencode:local", "-B", "-"], input=network_probe)
            for headers, expected in (({}, 200), ({"Origin": "https://untrusted.example.invalid"}, 403),
                                      ({"Host": "untrusted.example.invalid"}, 403)):
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
                connection.request("GET", "/", headers=headers)
                response = connection.getresponse()
                assert response.status == expected, "UI access check failed"
                if expected == 200:
                    assert "connect-src 'self'" in response.getheader("Content-Security-Policy", "")
                response.read()
                connection.close()
            # Use stdin instead of adding a test-script host mount to the agent.
            evidence = command(["docker", "exec", "-i", ids["opencode"], "python", "-B", "-", "--agent"],
                               input=Path(__file__).read_text(), timeout=100)
            assert (work / "desktop-smoke.txt").read_text() == "desktop-smoke-ok\n"
            print(evidence.strip())
            print("PASS: isolated agent, one workspace mount, limited credentials, UI origin/host guards and blocked egress.")
        finally:
            # Only the unique disposable test project is removed, never an operator's stack.
            subprocess.run(compose + ["-f", str(override), "down", "--volumes", "--remove-orphans"],
                           env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=45, check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", action="store_true")
    parser.add_argument("--agent", action="store_true")
    args = parser.parse_args()
    if args.backend:
        backend()
    elif args.agent:
        agent()
    else:
        run()
