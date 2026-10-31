"""Explicit interactive streaming smoke client. No network interface or third-party packages required."""
import argparse
import http.client
import json
from pathlib import Path
import socket
import sys
import time
import tomllib


def request(key, method, path, payload=None):
    connection = http.client.HTTPConnection("localhost", timeout=600)
    connection.sock = socket.socket(socket.AF_UNIX)
    connection.sock.settimeout(600)
    connection.sock.connect("/run/link-api/api.sock")
    connection.request(method, path, json.dumps(payload).encode() if payload else None,
        {"Authorization": "Bearer " + key, "Content-Type": "application/json"})
    response = connection.getresponse()
    if response.status != 200:
        connection.close()
        raise RuntimeError("k-lani-link returned HTTP " + str(response.status))
    return connection, response


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--models", action="store_true")
    parser.add_argument("--prompt", default="Write a short Python function that adds two numbers.")
    args = parser.parse_args()
    if {name for _, name in socket.if_nameindex()} != {"lo"}:
        raise RuntimeError("this example must run with network_mode: none")
    key = tomllib.loads(Path("/run/secrets/client.toml").read_text())["api_key"]
    deadline = time.monotonic() + 120
    while True:
        try:
            connection, response = request(key, "GET", "/health")
            health = json.load(response)
            connection.close()
            if health.get("connected"):
                break
            if health.get("new_code_required") or health.get("pairing_rejected"):
                raise RuntimeError("fresh pairing credentials required; no inference was retried")
        except OSError:
            pass
        if time.monotonic() >= deadline:
            raise TimeoutError("the peer tunnel is not ready")
        time.sleep(0.5)
    connection, response = request(key, "GET", "/v1/models")
    models = json.load(response)["data"]
    connection.close()
    if args.models:
        print(json.dumps(models, indent=2))
        return
    if not models:
        raise RuntimeError("backend did not offer a model")
    connection, response = request(key, "POST", "/v1/chat/completions", {
        "model": models[0]["id"], "messages": [{"role": "user", "content": args.prompt}],
        "max_tokens": 512, "stream": True, "stream_options": {"include_usage": True}})
    channel = None
    try:
        while True:
            line = response.readline(1024 * 1024)
            if not line:
                raise RuntimeError("stream ended without [DONE]")
            if not line.startswith(b"data: "):
                continue
            data = line[6:].strip()
            if data == b"[DONE]":
                break
            event = json.loads(data)
            if event.get("error"):
                raise RuntimeError("backend reported a generation error")
            for choice in event.get("choices", []):
                delta = choice.get("delta") or {}
                for name in ("reasoning_content", "content"):
                    value = delta.get(name)
                    if isinstance(value, str) and value:
                        if channel != name:
                            print("\n[thinking]" if name == "reasoning_content" else "\n[answer]", flush=True)
                            channel = name
                        print(value, end="", flush=True)
        print()
    finally:
        connection.close()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, http.client.HTTPException) as error:
        # This explicitly invoked demo displays a status; k-lani-link itself remains log-free.
        print(str(error), file=sys.stderr)
        raise SystemExit(1) from None
