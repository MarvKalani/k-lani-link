"""Codex -> local Unix API -> encrypted WebRTC -> isolated Basalt; synthetic files only."""
import argparse
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[2]
BINARY = ROOT / "target/release/k-lani-link"
FILES = Path(__file__).resolve().parent


def run(args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, check=True, timeout=120, **kwargs).stdout


def request(path, route, key=None):
    connection = http.client.HTTPConnection("localhost", timeout=3)
    connection.sock = socket.socket(socket.AF_UNIX)
    connection.sock.settimeout(3)
    connection.sock.connect(str(path))
    try:
        connection.request("GET", route, headers={"Authorization": "Bearer " + key} if key else {})
        response = connection.getresponse()
        assert response.status == 200
        return json.loads(response.read())
    finally:
        connection.close()


def wait_for(predicate, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, AssertionError, http.client.HTTPException):
            pass
        time.sleep(0.2)
    raise TimeoutError("expected service state was not reached")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex-binary", type=Path, required=True, help="Native Linux Codex binary, not its Node wrapper")
    parser.add_argument("--mode", choices=("cli", "app-server"), default="app-server")
    parser.add_argument("--real", action="store_true", help="Load the real GPU model instead of scripted model output")
    parser.add_argument("--model-file", type=Path, required=True)
    parser.add_argument("--context", type=int, default=32768)
    parser.add_argument("--timeout", type=int, default=900)
    args = parser.parse_args()
    binary = args.codex_binary.resolve(strict=True)
    assert binary.read_bytes()[:4] == b"\x7fELF", "use the native Codex executable"
    version = run([str(binary), "--version"]).strip()
    with tempfile.TemporaryDirectory(prefix="link-codex-test-") as directory:
        work = Path(directory)
        runtime, api, workspace = work / "runtime", work / "api", work / "workspace"
        for path in (runtime, api, workspace):
            path.mkdir(mode=0o700)
        seed = "link-" + uuid.uuid4().hex
        (workspace / "seed.txt").write_text(seed + "\n")
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            signal_port = probe.getsockname()[1]
        run([str(BINARY), "init", "--dir", str(work / "pair"), "--signaling-url", f"http://127.0.0.1:{signal_port}"])
        for node in ("host", "client"):
            path = work / "pair" / (node + ".toml")
            content = path.read_text()
            content = 'pairing_state_dir = ' + json.dumps(str(work / (node + "-state"))) + "\n" + content
            if node == "host":
                content = 'upstream_socket = ' + json.dumps(str(runtime / "basalt.sock")) + "\n" + content
                content = content.replace('upstream = "http://127.0.0.1:8080"', 'upstream = "http://localhost"')
            path.write_text(content)
        client_config = work / "pair/client.toml"
        key = tomllib.loads(client_config.read_text())["api_key"]
        basalt_config = work / "basalt.json"
        basalt_config.write_text(json.dumps({"server": {"model": "/models/model.basalt", "privacy": True,
            "vision": "off", "host": "127.0.0.1"}, "engine": {"max-context": args.context, "kv": "int8"}}))
        names = ["link-codex-basalt-" + uuid.uuid4().hex[:10], "link-codex-agent-" + uuid.uuid4().hex[:10]]
        children = []
        common = ["--network", "none", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
                  "--log-driver", "none", "--read-only", "--ulimit", "core=0"]
        try:
            backend = ["docker", "run", "-d", "--name", names[0], *common,
                "--tmpfs", "/tmp:uid=1000,gid=1000,mode=1777", "--tmpfs", "/home/link/.cache:uid=1000,gid=1000,mode=0700",
                "-v", str(runtime) + ":/run/link", "-v", str(basalt_config) + ":/run/secrets/basalt.json:ro"]
            if args.real:
                backend += ["--gpus", "all", "--memory", "16g", "--memory-swap", "16g", "-v", str(args.model_file.resolve(strict=True)) + ":/models/model.basalt:ro",
                    "k-lani-link-basalt:local", "--reasoning-budget-tokens", "1024"]
            else:
                backend += ["-v", str(FILES / "mock_backend.py") + ":/test/mock_backend.py:ro",
                    "-v", str(workspace / "seed.txt") + ":/test/seed.txt:ro", "--entrypoint", "/opt/basalt/.venv/bin/python",
                    "k-lani-link-basalt:local", "-B", "/test/mock_backend.py"]
            run(backend)
            print("Started isolated Basalt (" + ("GPU model" if args.real else "scripted model") + ").", flush=True)
            wait_for(lambda: request(runtime / "basalt.sock", "/health")["link_privacy"]["application_logs"] is False, args.timeout)
            for argv in (["signal", "--listen", f"127.0.0.1:{signal_port}", "--token-file", str(work / "pair/signaling-token.txt")],
                         ["host", "--config", str(work / "pair/host.toml")],
                         ["client", "--config", str(client_config), "--api-socket", str(api / "api.sock")]):
                children.append(subprocess.Popen([str(BINARY), *argv], stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            wait_for(lambda: request(api / "api.sock", "/health", key)["connected"], 45)
            children[0].terminate()
            children[0].wait(timeout=5)
            model = request(api / "api.sock", "/v1/models", key)["data"][0]["id"]
            profile = work / "link.config.toml"
            run([str(BINARY), "codex-profile", "--config", str(client_config), "--output", str(profile),
                 "--model", model, "--context", str(args.context)])
            print("Encrypted peers ready; signaling stopped; running " + version + " " + args.mode + ".", flush=True)
            command = ["docker", "run", "--rm", "--name", names[1], *common,
                "--tmpfs", "/home/link/.codex:uid=1000,gid=1000,mode=0700", "--tmpfs", "/tmp:uid=1000,gid=1000,mode=1777",
                "-v", str(profile) + ":/home/link/.codex/config.toml:ro",
                "-v", str(api / "api.sock") + ":/run/link/api.sock:ro", "-v", str(workspace) + ":/work",
                "-v", str(binary) + ":/usr/local/bin/codex:ro", "-v", str(FILES) + ":/test:ro",
                "-e", "LINK_TEST_MODEL=" + model, "-w", "/work"]
            for path in (BINARY.resolve(), client_config, profile.with_suffix(".models.json")):
                command += ["-v", f"{path}:{path}:ro"]
            command += ["--entrypoint", "python", "k-lani-link-host:local", "-B", "/test/bridge.py",
                        "python", "-B", "/test/agent_check.py", args.mode]
            agent = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout)
            assert agent.returncode == 0, agent.stderr[-5000:]
            details = json.loads(agent.stdout)
            assert (workspace / "result.txt").read_text() == seed + "\nverified\n"
            print("PASS: " + json.dumps({"codex": version, "model": model, "real_inference": args.real,
                "offline_agent": True, "offline_basalt": True, "signaling_detached": True, **details}), flush=True)
        finally:
            for name in names:
                subprocess.run(["docker", "rm", "-f", name], capture_output=True, timeout=20)
            for child in children:
                if child.poll() is None:
                    child.terminate()
                out, err = child.communicate(timeout=10)
                assert not out and not err, "k-lani-link produced application output"


if __name__ == "__main__":
    main()
