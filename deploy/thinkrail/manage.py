"""Start, inspect, measure and stop the private ThinkRail lab. Operates only its Compose project."""
import argparse
from datetime import datetime, timezone
import json
import http.client
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from prepare import create_pair

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def run(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, text=True, **kwargs).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("up", "new-pair", "select-model", "status", "record", "stop-recording", "down"))
    parser.add_argument("--root", type=Path, default=ROOT / ".thinkrail-test")
    parser.add_argument("--duration", type=float, default=14400)
    parser.add_argument("--output", type=Path, help="New resource recording directory for the record action")
    parser.add_argument("--backend-only", action="store_true", help="Start only signaling, peers and Basalt; omit ThinkRail and its UI")
    parser.add_argument("--preset", choices=("coder", "nvfp4"))
    args = parser.parse_args()
    if args.action == "select-model" and not args.preset:
        parser.error("select-model requires --preset")
    root = args.root.resolve(strict=True)
    compose = ["docker", "compose", "--env-file", str(root / "compose.env"), "-f", str(HERE / "compose.yaml")]
    if (root / "no-swap.profile").is_file():
        compose += ["-f", str(HERE / "compose.no-swap.yaml")]
    managed = (root / "managed.profile").is_file()
    if managed:
        compose += ["-f", str(HERE / "compose.managed.yaml")]
    if args.action == "select-model":
        if not managed:
            parser.error("select-model needs the managed profile; model files must be allowlisted before startup")
        print(run([*compose, "exec", "-T", "client", "/usr/local/bin/k-lani-link", "model",
            "--config", "/run/secrets/client.toml", "--api-socket", "/run/link-api/api.sock", "--load", args.preset]))
        return
    if (root / "no-swap.profile").is_file():
        if args.action in ("up", "new-pair"):
            if len(Path("/proc/swaps").read_text().splitlines()) != 1:
                raise RuntimeError("the selected test profile requires host swap to be completely disabled")
            running = run([*compose, "ps", "--status", "running", "-q", "basalt"]).strip()
            available = next(int(line.split()[1]) * 1024 for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemAvailable:"))
            if not running and available < 6 * 1024**3:
                raise RuntimeError("the no-swap test needs at least 6 GiB available RAM before starting Basalt")
    pid_file = root / "metrics/recorder.pid"
    if args.action in ("down", "stop-recording", "new-pair") and pid_file.exists():
        pid = int(pid_file.read_text())
        try:
            command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        except FileNotFoundError:
            command = []
        if any(item.endswith(b"thinkrail/metrics.py") for item in command):
            os.kill(pid, signal.SIGTERM)
            for _ in range(50):
                if not pid_file.exists():
                    break
                time.sleep(0.1)
        else:
            raise RuntimeError("recorder PID is stale; no process was stopped")
    if args.action == "down":
        print(run([*compose, "down", "--timeout", "3"]))
        return
    if args.action == "stop-recording":
        print("Resource recording stopped.")
        return
    if args.action == "status":
        print(run([*compose, "ps", "--all", "--format", "{{.Service}} {{.State}} {{.Status}} "]))
        return
    if args.action == "record":
        if pid_file.exists():
            raise RuntimeError("a resource recorder is already registered")
        record = args.output.resolve() if args.output else root / "measurements" / datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        no_swap = ["--require-no-swap"] if (root / "no-swap.profile").is_file() else []
        extra = ["--hardware-extra", str(root / "hardware-extra.json")] if (root / "hardware-extra.json").is_file() else []
        process = subprocess.Popen([sys.executable, "-B", str(HERE / "metrics.py"), "--live", str(root / "metrics"),
            "--output", str(record), "--duration", str(args.duration), *no_swap, *extra], stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        time.sleep(1)
        if process.poll() is not None:
            raise RuntimeError("resource recorder failed to start")
        print("Recording resources to " + str(record))
        return
    if args.action == "new-pair":
        # Explicit operator action creates a fresh code. Consumed-code volumes are preserved.
        stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
        fresh = root / ("pair-new-" + stamp)
        import tomllib
        current = tomllib.loads((root / "pair/client.toml").read_text())
        create_pair(fresh, concurrency=current.get("max_concurrent_requests", 1),
            queued=current.get("max_queued_requests", 0), managed=managed)
        run([*compose, "down", "--timeout", "3"])
        (root / "pair").rename(root / ("pair-consumed-" + stamp))
        fresh.rename(root / "pair")
    services = ["guard", "signal", "basalt", "host", "client"] if args.backend_only else []
    run([*compose, "up", "-d", "--no-build", *services])
    probe = """import http.client,socket,tomllib,json
from pathlib import Path
key=tomllib.loads(Path('/run/secrets/client.toml').read_text())['api_key']
c=http.client.HTTPConnection('localhost',timeout=3)
c.sock=socket.socket(socket.AF_UNIX);c.sock.settimeout(3);c.sock.connect('/run/link-api/api.sock')
c.request('GET','/health',headers={'Authorization':'Bearer '+key})
print(c.getresponse().read().decode());c.close()
"""
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        try:
            health = json.loads(run([*compose, "exec", "-T", "client", "python", "-B", "-c", probe]))
            if health.get("connected"):
                run([*compose, "stop", "--timeout", "1", "signal"])
                if args.backend_only:
                    print("Encrypted tunnel ready; signaling stopped. Backend available through the private k-lani-link Unix socket.")
                    return
                ui_deadline = time.monotonic() + 90
                while time.monotonic() < ui_deadline:
                    connection = http.client.HTTPConnection("127.0.0.1", 24242, timeout=2)
                    try:
                        connection.request("GET", "/")
                        response = connection.getresponse()
                        if response.status == 200:
                            break
                    except (OSError, http.client.HTTPException):
                        pass
                    finally:
                        connection.close()
                    time.sleep(0.5)
                else:
                    raise TimeoutError("tunnel established, but ThinkRail UI did not become ready")
                print("Encrypted tunnel ready; signaling stopped. ThinkRail: http://127.0.0.1:24242/")
                return
            if health.get("new_code_required") or health.get("pairing_rejected"):
                raise RuntimeError("new pairing credentials required; no inference was retried")
        except (subprocess.CalledProcessError, json.JSONDecodeError):
            pass
        time.sleep(0.25)
    raise TimeoutError("k-lani-link peers did not establish the encrypted tunnel")


if __name__ == "__main__":
    main()
