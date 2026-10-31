"""Manage the isolated OpenCode client and its detached resource recorder; the shared Basalt lab stays separate."""
import argparse
from datetime import datetime, timezone
import http.client
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def run(command):
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout


def no_swap():
    if len(Path("/proc/swaps").read_text().splitlines()) != 1:
        raise RuntimeError("host swap must be completely disabled before this test")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("up", "status", "live", "record", "stop-recording", "down"))
    parser.add_argument("--root", type=Path, default=ROOT / ".opencode-test")
    parser.add_argument("--duration", type=float, default=14400)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    compose = ["docker", "compose", "--env-file", str(root / "compose.env"), "-f", str(HERE / "compose.yaml")]
    if (root / "craft.profile").is_file():
        compose += ["-f", str(HERE / "compose.craft.yaml")]
    pid_file = root / "metrics/recorder.pid"
    if args.action in ("down", "stop-recording") and pid_file.exists():
        pid = int(pid_file.read_text())
        command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        if not any(part.endswith(b"thinkrail/metrics.py") for part in command):
            raise RuntimeError("stale recorder PID; no process stopped")
        os.kill(pid, signal.SIGTERM)
        deadline = time.monotonic() + 15
        while pid_file.exists() and time.monotonic() < deadline:
            time.sleep(0.1)
        if pid_file.exists():
            raise TimeoutError("resource recorder has not finished flushing")
    if args.action == "stop-recording":
        print("OpenCode resource recording stopped.")
        return
    if args.action == "down":
        print(run([*compose, "down", "--timeout", "5"]))
        return
    if args.action == "status":
        print(run([*compose, "ps", "--all"]))
        return
    no_swap()
    if args.action in ("record", "live"):
        if pid_file.exists():
            raise RuntimeError("a resource recorder is already registered")
        output = args.output.resolve() if args.output else root / "measurements" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        extra = ["--hardware-extra", str(root / "hardware-extra.json")] if (root / "hardware-extra.json").is_file() else []
        recording = ["--output", str(output)] if args.action == "record" else []
        process = subprocess.Popen([sys.executable, "-B", str(HERE.parent / "thinkrail/metrics.py"),
            "--agent", "opencode", "--project", "k-lani-opencode-test", "--backend-project", "k-lani-thinkrail-test",
            "--require-no-swap", "--live", str(root / "metrics"), "--duration", str(args.duration), *recording, *extra],
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        time.sleep(1)
        if process.poll() is not None:
            raise RuntimeError("resource recorder failed to start")
        if args.action == "record":
            (root / "metrics/recording.json").write_text(json.dumps({"directory": str(output), "pid": process.pid}))
        print("Recording resources to " + str(output) if args.action == "record" else "Live metrics enabled; no measurement history is recorded.")
        return
    backend = json.loads(run(["docker", "inspect", "k-lani-thinkrail-test-basalt-1"]))[0]
    if backend.get("State", {}).get("Health", {}).get("Status") != "healthy":
        raise RuntimeError("start the shared Basalt/k-lani-link lab first")
    run([*compose, "up", "-d", "--no-build"])
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        connection = http.client.HTTPConnection("127.0.0.1", 24244, timeout=2)
        try:
            connection.request("GET", "/")
            response = connection.getresponse()
            if response.status == 200:
                print("OpenCode UI: http://127.0.0.1:24244/ (shared k-lani-link tunnel; swap disabled)")
                return
        except (OSError, http.client.HTTPException):
            pass
        finally:
            connection.close()
        time.sleep(0.5)
    raise TimeoutError("OpenCode did not become ready")


if __name__ == "__main__":
    main()
