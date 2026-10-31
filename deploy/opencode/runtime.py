"""Run the pinned OpenCode server with an k-lani-link-only model provider inside network=none."""
import errno
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import stat
import subprocess
import threading
import time
import tomllib
from bridge import accept, forward


def model_from_backend(key):
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        client = http.client.HTTPConnection("localhost", timeout=3)
        try:
            client.sock = socket.socket(socket.AF_UNIX)
            client.sock.settimeout(3)
            client.sock.connect("/run/link-api/api.sock")
            client.request("GET", "/v1/models", headers={"Authorization": "Bearer " + key})
            response = client.getresponse()
            payload = response.read(65537)
            if response.status == 200 and len(payload) <= 65536:
                models = json.loads(payload)["data"]
                if len(models) != 1 or models[0].get("meta", {}).get("n_ctx", 0) < 262144:
                    raise ValueError("one backend model with at least 256K context is required")
                return models[0]["id"]
        except (OSError, http.client.HTTPException):
            pass
        finally:
            client.close()
        time.sleep(0.5)
    raise TimeoutError("k-lani-link model discovery did not complete")


def configuration(model, craft=False):
    config = {"model": "link/basalt", "default_agent": "build", "update": "disable", "share": "disabled",
        "warming": False, "snapshots": False, "formatter": False, "lsp": False,
        "providers": {"link": {"name": "Basalt via k-lani-link", "env": ["LINK_API_KEY"],
            "package": "@opencode/ai/providers/openai-compatible", "settings": {"baseURL": "http://127.0.0.1:8787/v1"},
            "compatibility": {"reasoningField": "reasoning_content", "maxTokensField": "max_tokens", "streamUsage": True},
            "body": {"store": False, "reasoning_effort": "medium"},
            "models": {"basalt": {"modelID": model, "name": model + " · k-lani-link · 256K",
                "limit": {"context": 262144, "output": 16384},
                "capabilities": {"tools": True, "input": ["text"], "output": ["text"], "reasoning": {"supported": True}},
                "cost": {"input": 0, "output": 0, "cache": {"read": 0, "write": 0}}}}}},
        "policies": [{"action": "provider.use", "resource": "*", "effect": "deny"},
            {"action": "provider.use", "resource": "link", "effect": "allow"}],
        "permissions": [{"action": "websearch", "resource": "*", "effect": "deny"},
            {"action": "webfetch", "resource": "*", "effect": "deny"},
            {"action": "task", "resource": "*", "effect": "deny"}]}
    if craft:
        config["mcp"] = {"servers": {
            "wordcraft": {"type": "local", "codemode": False, "cwd": "/work/demo",
                "command": ["wordcraft-cli", "mcp", "--author", "Basalt via k-lani-link",
                    "--automation-read-root", "/work/demo", "--automation-write-root", "/work/demo/output"]},
            "gridcraft": {"type": "local", "codemode": False, "cwd": "/work/demo",
                "command": ["gridcraft-cli", "mcp"]}}}
    return config


def ui_connection(client):
    with client:
        try:
            upstream = socket.create_connection(("127.0.0.1", 4096), timeout=3)
        except OSError:
            return
        with upstream:
            upstream.settimeout(None)
            thread = threading.Thread(target=forward, args=(client, upstream), daemon=True)
            thread.start()
            forward(upstream, client)
            thread.join(timeout=1)


def serve_ui(listener):
    while True:
        client, _ = listener.accept()
        threading.Thread(target=ui_connection, args=(client,), daemon=True).start()


def main():
    if {name for _, name in socket.if_nameindex()} != {"lo"} or len(Path("/proc/swaps").read_text().splitlines()) != 1:
        return 2
    os.umask(0o077)
    key = tomllib.loads(Path("/run/secrets/client.toml").read_text())["api_key"]
    model = model_from_backend(key)
    os.environ["LINK_API_KEY"] = key
    os.environ["OPENCODE_PASSWORD"] = Path("/run/secrets/server-password.txt").read_text().strip()
    config = Path.home() / ".config/opencode/opencode.json"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(json.dumps(configuration(model, os.environ.get("LINK_CRAFT_TOOLS") == "1"), indent=2))
    with socket.socket() as api, socket.socket(socket.AF_UNIX) as ui:
        api.bind(("127.0.0.1", 8787)); api.listen(8)
        threading.Thread(target=accept, args=(api, "/run/link-api/api.sock"), daemon=True).start()
        path = Path("/run/opencode-ui/http.sock")
        if path.exists():
            info = path.lstat()
            if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid():
                return 2
            with socket.socket(socket.AF_UNIX) as probe:
                try:
                    probe.connect(str(path))
                except OSError as error:
                    if error.errno != errno.ECONNREFUSED:
                        return 2
                else:
                    return 2
            path.unlink()
        ui.bind(str(path)); path.chmod(0o600); ui.listen(16)
        threading.Thread(target=serve_ui, args=(ui,), daemon=True).start()
        process = subprocess.Popen(["opencode", "serve", "--hostname", "127.0.0.1", "--port", "4096",
            "--cors", "http://127.0.0.1:24244", "--cors", "http://localhost:24244", "--log-level", "none"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for sig in (signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, lambda *_: process.terminate())
        try:
            return process.wait()
        finally:
            path.unlink(missing_ok=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError):
        raise SystemExit(2) from None
