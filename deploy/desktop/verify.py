"""Fixed-output checks inside the network-isolated OpenCode container."""
import argparse
import base64
import http.client
import os
from pathlib import Path
import socket
import tempfile
import tomllib


def health():
    password = Path("/run/secrets/server-password.txt").read_text().strip()
    auth = base64.b64encode(("opencode:" + password).encode()).decode()
    connection = http.client.HTTPConnection("127.0.0.1", 4096, timeout=2)
    try:
        connection.request("GET", "/global/health", headers={"Authorization": "Basic " + auth})
        return connection.getresponse().status == 200
    finally:
        connection.close()


def boundary():
    if {name for _, name in socket.if_nameindex()} != {"lo"} or os.getuid() != 1000:
        raise ValueError("unexpected network or user")
    if len(Path("/proc/swaps").read_text().splitlines()) != 1:
        raise ValueError("VM swap must be disabled")
    status = dict(line.split(":", 1) for line in Path("/proc/self/status").read_text().splitlines() if ":" in line)
    if int(status["CapEff"].strip(), 16) != 0 or status["NoNewPrivs"].strip() != "1":
        raise ValueError("unexpected process privileges")
    credentials = tomllib.loads(Path("/run/secrets/client.toml").read_text())
    if set(credentials) != {"api_key"}:
        raise ValueError("agent has excessive credentials")
    if any(Path(p).exists() for p in ("/var/run/docker.sock", "/run/docker.sock", "/mnt/c", "/mnt/d", "/host")):
        raise ValueError("unexpected host access")
    for address in ("1.1.1.1", "8.8.8.8"):
        with socket.socket() as connection:
            connection.settimeout(0.5)
            if connection.connect_ex((address, 443)) == 0:
                raise ValueError("unexpected egress")
    with tempfile.TemporaryFile(dir="/work") as output:
        output.write(b"desktop-boundary-probe")
        output.seek(0)
        if output.read() != b"desktop-boundary-probe":
            raise ValueError("workspace is not writable")
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(2)
        connection.connect("/run/link-api/api.sock")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--health", action="store_true")
    args = parser.parse_args()
    try:
        if args.health:
            raise SystemExit(0 if health() else 1)
        boundary()
    except (OSError, ValueError, http.client.HTTPException):
        raise SystemExit("Desktop isolation check failed.") from None
    print("Desktop isolation check passed: loopback only, no capabilities, limited credentials and writable workspace.")
