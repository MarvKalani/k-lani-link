"""Measure complete GET /v1/models round trips without running model inference or recording payloads."""
import argparse
from datetime import datetime, timezone
import http.client
import json
from pathlib import Path
import socket
import statistics
import time
import tomllib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--key-file", type=Path)
    parser.add_argument("--samples", type=int, default=200)
    parser.add_argument("--label", default="application round trip")
    args = parser.parse_args()
    if not 10 <= args.samples <= 10000:
        parser.error("samples must be between 10 and 10000")
    headers = {}
    if args.key_file:
        headers["Authorization"] = "Bearer " + tomllib.loads(args.key_file.read_text())["api_key"]
    class UnixHTTPConnection(http.client.HTTPConnection):
        def connect(self):
            self.sock = socket.socket(socket.AF_UNIX)
            self.sock.settimeout(self.timeout)
            self.sock.connect(args.socket)

    connection = UnixHTTPConnection("localhost", timeout=10)
    values = []
    try:
        for index in range(args.samples + 10):
            started = time.perf_counter_ns()
            connection.request("GET", "/v1/models", headers=headers)
            response = connection.getresponse()
            body = response.read(65537)
            elapsed = (time.perf_counter_ns() - started) / 1e6
            if response.status != 200 or len(body) > 65536 or not json.loads(body).get("data"):
                raise RuntimeError("model discovery did not return a valid response")
            if index >= 10:
                values.append(elapsed)
            time.sleep(0.02)
    finally:
        connection.close()
    ordered = sorted(values)
    print(json.dumps({"label": args.label, "utc": datetime.now(timezone.utc).isoformat(),
        "scope": "complete authenticated HTTP request/response including backend model-list handling; not pure network RTT or time to first token",
        "route": "GET /v1/models", "samples": len(values), "warmup": 10,
        "unit": "ms", "min": round(min(values), 3), "median": round(statistics.median(values), 3),
        "p95": round(ordered[(95 * len(ordered) + 99) // 100 - 1], 3), "max": round(max(values), 3)}, indent=2))


if __name__ == "__main__":
    main()
