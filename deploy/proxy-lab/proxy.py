"""mitmproxy addon: exact-domain policy, controlled faults, metadata-only evidence."""
import asyncio
import base64
import hmac
import json
import os
from pathlib import Path
import sys
import time
from urllib.parse import urlsplit

from mitmproxy import http

sys.path.insert(0, "/lab")
from policy import payload_summary, permitted, request_status


class Lab:
    def __init__(self):
        self.used_faults = set()

    def policy(self):
        try:
            value = json.loads(Path("/control/policy.json").read_text())
            return value if isinstance(value, dict) else {}
        except (OSError, ValueError):
            return {}

    def event(self, event, **fields):
        # Deliberately no full URLs, headers, tokens, pairing codes, bodies or error strings.
        with Path("/audit/events.jsonl").open("a") as output:
            output.write(json.dumps({"time": time.time(), "event": event, **fields}) + "\n")

    def running(self):
        self.event("ready")

    def tls_established_client(self, data):
        self.event("client_tls", version=data.conn.tls_version,
                   alpn=(data.conn.alpn or b"").decode("ascii", "replace"))

    def tls_established_server(self, data):
        self.event("server_tls", version=data.conn.tls_version)

    def tls_failed_client(self, data):
        self.event("client_tls_failed")

    def tls_failed_server(self, data):
        self.event("server_tls_failed")

    def http_connect(self, flow):
        policy = self.policy()
        if not permitted(policy, flow.request.host, flow.request.port):
            flow.response = http.Response.make(403, b"Lab domain policy denied\n")
            self.event("connect_denied")
            return
        if policy.get("require_auth"):
            try:
                credentials = Path("/control/proxy-auth").read_text().strip()
            except OSError:
                credentials = ""
            expected = "Basic " + base64.b64encode(credentials.encode()).decode()
            if not credentials or not hmac.compare_digest(flow.request.headers.get("proxy-authorization", ""), expected):
                flow.response = http.Response.make(
                    407, b"Lab proxy authentication required\n",
                    {"Proxy-Authenticate": 'Basic realm="k-lani-proxy-lab"'},
                )
                self.event("proxy_auth_denied")
                return
            self.event("proxy_auth_accepted")
        self.event("connect_allowed")

    async def request(self, flow):
        policy = self.policy()
        status = request_status(policy, flow.request.host, flow.request.port,
                                flow.request.method, flow.request.path,
                                flow.request.headers.get("content-type", ""), flow.request.scheme)
        self.event("request", method=flow.request.method,
                   http_version=flow.request.http_version,
                   tls=flow.client_conn.tls_established,
                   content_type=flow.request.headers.get("content-type", ""),
                   blocked=status is not None,
                   **payload_summary(flow.request.raw_content or b"",
                                     flow.request.headers.get("content-type", "")))
        if status:
            flow.response = http.Response.make(status, b"Lab request policy denied\n")
        elif policy.get("mode") == "buffered":
            await asyncio.sleep(0.08)

    async def response(self, flow):
        if flow.request.method == "CONNECT":
            return
        policy = self.policy()
        body = flow.response.raw_content or b""
        self.event("response", status=flow.response.status_code,
                   **payload_summary(body, flow.response.headers.get("content-type", "")))
        mode = policy.get("mode")
        if mode == "buffered":
            await asyncio.sleep(0.08)
        fault = (mode, policy.get("generation", 0))
        if fault in self.used_faults or flow.response.status_code not in {200, 204}:
            return
        if (mode == "drop_once" and flow.request.method in {"PUT", "POST"}
                and flow.request.path.startswith("/v1/box/")):
            self.used_faults.add(fault)
            self.event("dropped_write_ack")
            flow.kill()
        if (mode == "tamper_once" and flow.request.method == "GET"
                and urlsplit(flow.request.path).path.endswith("/h2c") and len(body) > 8):
            self.used_faults.add(fault)
            # Corrupt a sealed record after the relay has returned it.
            flow.response.content = body[:-1] + bytes([body[-1] ^ 1])
            self.event("tampered_record")

    def error(self, flow):
        self.event("flow_error")


os.umask(0o077)
addons = [Lab()]
