"""Inside the lab endpoint image: synthetic backend and probes of the real CLI."""
import argparse
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import threading
import time
import tomllib
import uuid

STATE = Path("/state")


def save(path, value):
    path = Path(path)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def config():
    path = Path(os.environ.get("LAB_CLIENT_CONFIG", str(STATE / "client.toml")))
    return tomllib.loads(path.read_text())


def api_connection(timeout):
    host, port = config()["listen"].rsplit(":", 1)
    return http.client.HTTPConnection(host, int(port), timeout=timeout)


def health():
    connection = api_connection(2)
    try:
        connection.request("GET", "/health", headers={"Authorization": "Bearer " + config()["api_key"]})
        response = connection.getresponse()
        return json.loads(response.read())
    finally:
        connection.close()


def wait_state(seconds, field="connected"):
    deadline = time.monotonic() + seconds
    last = {}
    while time.monotonic() < deadline:
        try:
            last = health()
            if last.get(field):
                return last
        except (OSError, ValueError, http.client.HTTPException):
            pass
        time.sleep(0.1)
    return last


def probe(args):
    request_id = uuid.uuid4().hex
    question = "QUESTION_KLANI_PROXY_LAB_" + request_id
    answer = "ANSWER_KLANI_PROXY_LAB_" + request_id
    result = {"complete": False, "tokens": 0, "expected": args.tokens,
              "request_id": request_id, "done": False, "ordered": True}
    save(STATE / "progress.json", result)
    connection = api_connection(args.timeout)
    started = time.monotonic()
    try:
        body = json.dumps({"model": "proxy-lab-synthetic", "stream": True,
                           "messages": [{"role": "user", "content": question}],
                           "metadata": {"lab_id": request_id, "tokens": args.tokens,
                                        "delay": args.delay}})
        connection.request("POST", "/v1/chat/completions", body,
                           {"Authorization": "Bearer " + config()["api_key"],
                            "Content-Type": "application/json"})
        response = connection.getresponse()
        result["http_status"] = response.status
        if response.status != 200:
            return result
        while True:
            line = response.readline(128 * 1024)
            if not line:
                break
            if not line.startswith(b"data: "):
                continue
            data = line[6:].strip()
            if data == b"[DONE]":
                if result["done"]:
                    result["ordered"] = False
                result["done"] = True
                continue
            value = json.loads(data)
            token = value["choices"][0]["delta"]["content"]
            n = result["tokens"]
            if result["done"] or token != answer + "_" + str(n):
                result["ordered"] = False
            result["tokens"] += 1
            if n == 0:
                result["first_token_ms"] = round((time.monotonic() - started) * 1000, 2)
            save(STATE / "progress.json", result)
        result["complete"] = result["done"] and result["ordered"] and result["tokens"] == args.tokens
    except (OSError, ValueError, KeyError, http.client.HTTPException) as error:
        result["error_type"] = type(error).__name__
    finally:
        connection.close()
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        save(STATE / "probe.json", result)
    return result


class Backend(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    lock = threading.Lock()
    calls = Counter()
    completed = 0
    cancelled = 0

    def log_message(self, *args):
        pass

    @classmethod
    def report(cls):
        save(STATE / "backend.json", {"calls": sum(cls.calls.values()),
                                     "duplicates": sum(max(0, n - 1) for n in cls.calls.values()),
                                     "completed": cls.completed, "cancelled": cls.cancelled})

    def do_GET(self):
        body = json.dumps({"link_privacy": {"version": 1, "application_logs": False,
                          "request_history": False, "diagnostics": False,
                          "remote_images": False}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self.send_error(404)
            return
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            body = bytearray()
            while True:
                length = int(self.rfile.readline(128).split(b";", 1)[0].strip(), 16)
                if not length:
                    while self.rfile.readline(4096).strip():
                        pass
                    break
                if len(body) + length > 256 * 1024:
                    self.send_error(413)
                    return
                body.extend(self.rfile.read(length))
                if self.rfile.read(2) != b"\r\n":
                    self.send_error(400)
                    return
        else:
            length = int(self.headers.get("Content-Length", "0"))
            body = self.rfile.read(length) if 0 < length < 256 * 1024 else b""
        if not body:
            self.send_error(413)
            return
        value = json.loads(body)
        metadata = value["metadata"]
        request_id = metadata["lab_id"]
        if value["messages"][0]["content"] != "QUESTION_KLANI_PROXY_LAB_" + request_id:
            self.send_error(400)
            return
        tokens = min(1000, max(1, int(metadata["tokens"])))
        delay = min(0.1, max(0.0, float(metadata["delay"])))
        with self.lock:
            self.calls[request_id] += 1
            self.report()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        try:
            for n in range(tokens):
                event = {"choices": [{"delta": {"content": "ANSWER_KLANI_PROXY_LAB_" + request_id + "_" + str(n)}}]}
                self.chunk(("data: " + json.dumps(event) + "\n\n").encode())
                time.sleep(delay)
            self.chunk(b"data: [DONE]\n\n")
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()
            with self.lock:
                type(self).completed += 1
                self.report()
        except (BrokenPipeError, ConnectionResetError):
            with self.lock:
                type(self).cancelled += 1
                self.report()

    def chunk(self, data):
        self.wfile.write(f"{len(data):x}\r\n".encode() + data + b"\r\n")
        self.wfile.flush()


def host():
    Backend.report()
    server = ThreadingHTTPServer(("127.0.0.1", 8080), Backend)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    child = subprocess.Popen(["k-lani-link", "host", "--config", "/state/host.toml"])
    def stop(*args):
        child.terminate()
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        return child.wait()
    finally:
        server.shutdown()


def connectivity(args):
    routes = Path("/proc/net/route").read_text().splitlines()[1:]
    checks = {"default_ipv4_route": any(line.split()[1] == "00000000" for line in routes)}
    for label, host, port in [("proxy_reachable", "proxy.corp.test", 8080),
                              ("relay_direct_reachable", args.relay_ip, 443),
                              ("model_direct_reachable", args.host_ip, 8080)]:
        try:
            with socket.create_connection((host, port), timeout=1):
                checks[label] = True
        except OSError:
            checks[label] = False
    return checks


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["host", "health", "wait", "wait-denied", "probe", "connectivity"])
    parser.add_argument("--timeout", type=float, default=15)
    parser.add_argument("--tokens", type=int, default=32)
    parser.add_argument("--delay", type=float, default=0.01)
    parser.add_argument("--relay-ip")
    parser.add_argument("--host-ip")
    args = parser.parse_args()
    if args.action == "host":
        return host()
    if args.action == "probe":
        result = probe(args)
    elif args.action == "connectivity":
        result = connectivity(args)
    elif args.action in {"wait", "wait-denied"}:
        result = wait_state(args.timeout, "access_denied" if args.action == "wait-denied" else "connected")
    else:
        result = health()
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    sys.exit(main())
