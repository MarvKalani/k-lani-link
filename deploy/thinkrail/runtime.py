"""Run ThinkRail/Pi with only local Unix-socket access to the UI gateway and k-lani-link."""
import json
import os
import errno
import http.client
from pathlib import Path
import shutil
import signal
import socket
import stat
import subprocess
import threading
import time
import tomllib
from bridge import accept, forward


def model_from_backend(key):
    deadline = time.monotonic() + 90
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


def expose_ui(listener):
    while True:
        client, _ = listener.accept()
        threading.Thread(target=ui_connection, args=(client,), daemon=True).start()


def ui_connection(client):
    with client:
        try:
            upstream = socket.create_connection(("127.0.0.1", 24242), timeout=5)
        except OSError:
            return
        with upstream:
            upstream.settimeout(None)
            upload = threading.Thread(target=forward, args=(client, upstream), daemon=True)
            upload.start()
            forward(upstream, client)
            upload.join(timeout=1)


def main():
    if os.environ.get("LINK_REQUIRE_NO_SWAP") == "1" and len(Path("/proc/swaps").read_text().splitlines()) != 1:
        return 2
    if {name for _, name in socket.if_nameindex()} != {"lo"}:
        return 2
    os.umask(0o077)
    recovered = Path.home() / ".link-recovery-complete"
    if not recovered.exists():
        for name in (".thinkrail", ".pi"):
            saved = Path("/run/thinkrail-recovery") / name
            if saved.is_dir():
                shutil.copytree(saved, Path.home() / name, dirs_exist_ok=True)
        recovered.touch(mode=0o600)
    config = tomllib.loads(Path("/run/secrets/client.toml").read_text())
    os.environ["LINK_API_KEY"] = config["api_key"]
    model_id = model_from_backend(config["api_key"])
    agent = Path.home() / ".pi/agent"
    agent.mkdir(parents=True, exist_ok=True)
    models = {"providers": {"link": {
        "baseUrl": "http://127.0.0.1:8787/v1", "api": "openai-completions", "apiKey": "$LINK_API_KEY",
        "models": [{"id": model_id, "name": "Basalt " + model_id + " · k-lani-link · 256K",
            "reasoning": True, "input": ["text"], "contextWindow": 262144, "maxTokens": 16384,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
            "compat": {"supportsStore": False, "supportsDeveloperRole": False, "maxTokensField": "max_tokens"}}]
    }}}
    (agent / "models.json").write_text(json.dumps(models, indent=2))
    (agent / "settings.json").write_text(json.dumps({"defaultProvider": "link", "defaultModel": model_id,
        "defaultThinkingLevel": "medium", "enabledModels": ["link/*"], "cacheWarming": "off",
        "retry": {"enabled": False}, "defaultTools": ["read", "bash", "edit", "write"]}, indent=2))
    app = Path.home() / ".thinkrail"
    app.mkdir(exist_ok=True)
    settings_path = app / "config.json"
    settings = json.loads(settings_path.read_text()) if settings_path.exists() else {}
    settings.update({"analyticsEnabled": False, "analyticsConsentConfirmed": True,
        "subagentsEnabled": False, "agentReviewEnabled": False, "reviewAutoFix": False,
        "jbcentralQuotaEnabled": False, "notificationsEnabled": False, "defaultEffort": "medium"})
    settings_path.write_text(json.dumps(settings))
    with socket.socket() as api, socket.socket(socket.AF_UNIX) as ui:
        api.bind(("127.0.0.1", 8787))
        api.listen(8)
        threading.Thread(target=accept, args=(api, "/run/link-api/api.sock"), daemon=True).start()
        ui_path = Path("/run/thinkrail-ui/http.sock")
        if ui_path.exists():
            previous = ui_path.lstat()
            if not stat.S_ISSOCK(previous.st_mode) or previous.st_uid != os.getuid():
                return 2
            with socket.socket(socket.AF_UNIX) as probe:
                try:
                    probe.connect(str(ui_path))
                except OSError as error:
                    if error.errno != errno.ECONNREFUSED:
                        return 2
                else:
                    return 2
            ui_path.unlink()
        ui.bind(str(ui_path))
        ui_path.chmod(0o600)
        ui.listen(16)
        threading.Thread(target=expose_ui, args=(ui,), daemon=True).start()
        process = subprocess.Popen(["thinkrail", "--no-open", "--no-analytics", "--host", "127.0.0.1",
            "--port", "24242", "/work/demo"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for sig in (signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, lambda *_: process.terminate())
        try:
            return process.wait()
        finally:
            ui_path.unlink(missing_ok=True)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError):
        raise SystemExit(2) from None
