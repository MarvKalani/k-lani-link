"""Offline Basalt model supervisor. Only operator-defined IDs cross its private socket."""
import http.client
import json
import os
from pathlib import Path
import re
import resource
import signal
import socket
import socketserver
import stat
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler

MAX_BODY = 1024
MAX_REPLY = 64 * 1024


def model_id(value):
    return isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,64}", value) is not None


def catalog_from_file(path):
    raw = Path(path).read_bytes()
    if len(raw) > MAX_REPLY:
        raise ValueError("catalog too large")
    doc = json.loads(raw)
    if not isinstance(doc, dict) or set(doc) - {"default", "models"}:
        raise ValueError("invalid catalog")
    rows = doc.get("models")
    if not isinstance(rows, list) or not 1 <= len(rows) <= 32:
        raise ValueError("invalid models")
    models = {}
    for row in rows:
        if not isinstance(row, dict) or set(row) - {"id", "path", "context_tokens", "parallel", "slot_full_context", "prefill", "reasoning_budget_tokens"}:
            raise ValueError("invalid model profile")
        name, path = row.get("id"), row.get("path")
        if not model_id(name) or name in models or not isinstance(path, str) or not Path(path).is_absolute() or not Path(path).is_file():
            raise ValueError("invalid local model")
        profile = dict(row)
        for key, default, low, high in [("context_tokens", 262144, 4096, 1048576), ("parallel", 1, 1, 8),
                ("prefill", 512, 128, 8192), ("reasoning_budget_tokens", 2048, 0, 65536)]:
            value = profile.setdefault(key, default)
            if type(value) is not int or not low <= value <= high:
                raise ValueError("invalid model setting")
        if type(profile.setdefault("slot_full_context", False)) is not bool:
            raise ValueError("invalid slot setting")
        models[name] = profile
    default = doc.get("default")
    if default is not None and default not in models:
        raise ValueError("unknown default model")
    return default, models


def remove_socket(path):
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid():
        raise ValueError("unsafe socket path")
    path.unlink()


def health(path):
    connection = http.client.HTTPConnection("localhost", timeout=1)
    try:
        connection.sock = socket.socket(socket.AF_UNIX)
        connection.sock.settimeout(1)
        connection.sock.connect(str(path))
        connection.request("GET", "/health")
        response = connection.getresponse()
        data = response.read(MAX_REPLY + 1)
        if response.status != 200 or len(data) > MAX_REPLY:
            return False
        data = json.loads(data)
        privacy = data.get("link_privacy", {})
        valid = privacy.get("version") == 1 and all(privacy.get(k) is False for k in (
            "application_logs", "request_history", "diagnostics", "remote_images"))
        return data if valid and data.get("loaded") is True else None
    except (OSError, ValueError, http.client.HTTPException):
        return False
    finally:
        connection.close()


class Manager:
    def __init__(self, models, runtime, program=None, metrics=False, timeout=180):
        self.models = models
        self.runtime = Path(runtime)
        self.socket = self.runtime / "basalt.sock"
        self.program = program or ["/opt/basalt/.venv/bin/python", "-B", "-m", "serve.server",
            "--engine-bin", "/opt/basalt/bin/basalt"]
        self.metrics = metrics
        self.timeout = timeout
        self.process = None
        self.active = None
        self.stopping = threading.Event()
        self.lock = threading.Lock()

    def snapshot(self):
        active = self.active if self.active is not None and self.process is not None and self.process.poll() is None and health(self.socket) else None
        return {"active_model": active, "models": [
            {"id": key, "context_tokens": p["context_tokens"], "parallel": p["parallel"],
             "context_per_slot": p["context_tokens"] if p["slot_full_context"] else p["context_tokens"] // p["parallel"],
             "loaded": key == active}
            for key, p in self.models.items()]}

    def stop_engine(self):
        process, self.process = self.process, None
        self.active = None
        if process is not None:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=5)
        remove_socket(self.socket)
        for name in ("performance.json", "performance.json.tmp"):
            path = self.runtime / name
            if path.is_file() and not path.is_symlink():
                path.unlink()

    def load(self, name):
        if name not in self.models:
            return 404, {"error": "unknown model ID"}
        if self.stopping.is_set():
            return 503, {"error": "backend is stopping"}
        if self.active == name and health(self.socket):
            return 200, self.snapshot()
        self.stop_engine()
        profile = self.models[name]
        args = [*self.program, "--model", profile["path"], "--alias", name,
            "--privacy", "--vision", "off", "--host", "127.0.0.1", "--unix-socket", str(self.socket),
            "--parallel", str(profile["parallel"]), "--max-context", str(profile["context_tokens"]),
            "--kv", "int8", "--mmap-experts", "--prefill", str(profile["prefill"]),
            "--reasoning-budget-tokens", str(profile["reasoning_budget_tokens"])]
        if profile["slot_full_context"]:
            args.append("--slot-full-context")
        if self.metrics:
            args += ["--privacy-metrics-file", str(self.runtime / "performance.json")]
        self.process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True)
        deadline = time.monotonic() + self.timeout
        while not self.stopping.is_set() and time.monotonic() < deadline and self.process.poll() is None:
            ready = health(self.socket)
            if ready:
                context = profile["context_tokens"] if profile["slot_full_context"] else profile["context_tokens"] // profile["parallel"]
                if ready.get("model") != name or ready.get("parallel") != profile["parallel"] or ready.get("max_context") != context:
                    self.stop_engine()
                    return 503, {"error": "backend could not provide the configured model, slots or context",
                        "actual_parallel": ready.get("parallel"), "actual_context": ready.get("max_context")}
                self.active = name
                return 200, self.snapshot()
            self.stopping.wait(0.1)
        exit_code = self.process.poll()
        self.stop_engine()
        return 503, {"error": "model failed to become ready; backend remains unloaded", "exit_code": exit_code}

    def command(self, body):
        if not isinstance(body, dict):
            return 400, {"error": "invalid model control command"}
        if body.get("action") == "load" and set(body) == {"action", "model"} and model_id(body["model"]):
            return self.load(body["model"])
        if body == {"action": "unload"}:
            self.stop_engine()
            return 200, self.snapshot()
        return 400, {"error": "invalid model control command"}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def reply(self, status, body):
        data = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path != "/v1/link/models":
            self.reply(404, {"error": "route not available"})
            return
        with self.server.manager.lock:
            self.reply(200, self.server.manager.snapshot())

    def do_POST(self):
        if self.path != "/v1/link/model":
            self.reply(404, {"error": "route not available"})
            return
        try:
            if self.headers.get("Transfer-Encoding"):
                raise ValueError()
            size = int(self.headers.get("Content-Length", "-1"))
            if not 0 < size <= MAX_BODY:
                raise ValueError()
            body = json.loads(self.rfile.read(size))
        except (ValueError, OSError):
            self.reply(400, {"error": "invalid model control command"})
            return
        with self.server.manager.lock:
            status, result = self.server.manager.command(body)
        self.reply(status, result)


class Server(socketserver.UnixStreamServer):
    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(5)
        return connection, address

    def handle_error(self, *_):
        pass


def main():
    # Never open a management TCP listener or permit a network-capable model process.
    sink = os.open(os.devnull, os.O_WRONLY)
    os.dup2(sink, 1)
    os.dup2(sink, 2)
    if sink not in (1, 2):
        os.close(sink)
    os.umask(0o077)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    if {name for _, name in socket.if_nameindex()} - {"lo"}:
        return 2
    if os.environ.get("LINK_REQUIRE_NO_SWAP") == "1" and len(Path("/proc/swaps").read_text().splitlines()) != 1:
        return 2
    default, models = catalog_from_file("/run/secrets/models.json")
    runtime = Path("/run/link")
    control = runtime / "control.sock"
    remove_socket(control)
    manager = Manager(models, runtime, metrics=os.environ.get("LINK_NUMERIC_METRICS") == "1")
    server = Server(str(control), Handler)
    server.manager = manager
    os.chmod(control, 0o600)
    signal.signal(signal.SIGTERM, lambda *_: manager.stopping.set())
    signal.signal(signal.SIGINT, lambda *_: manager.stopping.set())
    try:
        if default is not None:
            with manager.lock:
                manager.load(default)
        server.timeout = 0.5
        while not manager.stopping.is_set():
            server.handle_request()
    finally:
        server.server_close()
        manager.stop_engine()
        remove_socket(control)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        raise SystemExit(1) from None
