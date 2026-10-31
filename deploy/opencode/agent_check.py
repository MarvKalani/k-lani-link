"""Run inside the OpenCode container: synthetic agent/tool/stream test, output only aggregate evidence."""
from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import tomllib


def main():
    assert len(Path("/proc/swaps").read_text().splitlines()) == 1, "swap is enabled"
    os.environ["OPENCODE_PASSWORD"] = Path("/run/secrets/server-password.txt").read_text().strip()
    local_key = tomllib.loads(Path("/run/secrets/client.toml").read_text())["api_key"]
    def redact(value):
        return str(value).replace(local_key, "[redacted]").replace(os.environ["OPENCODE_PASSWORD"], "[redacted]")
    result = Path("/work/opencode-test.txt")
    assert not result.exists(), "test fixture already has a result; use a fresh workspace"
    seed = Path("/work/seed.txt").read_text()
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd="/work", text=True).strip()
    prompt = ("Lokaler k-lani-link-Funktionstest. Lies seed.txt. Erstelle ausschließlich opencode-test.txt mit genau zwei Zeilen: "
        "der ersten Zeile aus seed.txt und danach opencode-link-ok. Prüfe die Datei mit einem Shell-Befehl. "
        "Keine anderen Dateien ändern, keine Commits, keine Unteragenten und keine Netzwerkwerkzeuge. "
        "Antworte zum Abschluss mit OPENCODE_LINK_OK.")
    start = time.monotonic()
    process = subprocess.Popen(["opencode", "run", "--server", "http://127.0.0.1:4096", "--format", "json",
        "--thinking", "--auto", "--model", "link/basalt", "--title", "k-lani-link OpenCode no-swap check",
        "--log-level", "none", prompt], cwd="/work", stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    out, err = process.communicate(timeout=600)
    events, errors = [], []
    for line in out.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        events.append(event)
        if event.get("type") == "error":
            errors.append(redact(event.get("error", {}))[:400])
    counts = Counter(event.get("type", "unknown") for event in events)
    reasoning = sum(event.get("type") == "reasoning" or event.get("part", {}).get("type") == "reasoning" for event in events)
    tools = [event.get("part", {}).get("tool") for event in events if event.get("type") == "tool_use" or event.get("part", {}).get("type") == "tool"]
    verified = result.is_file() and result.read_text() == seed.splitlines()[0] + "\nopencode-link-ok\n"
    unchanged_head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd="/work", text=True).strip() == head
    report = {"exit_code": process.returncode, "elapsed_seconds": round(time.monotonic() - start, 2),
        "event_types": dict(counts), "reasoning_events": reasoning, "tools": tools,
        "file_verified": verified, "seed_unchanged": Path('/work/seed.txt').read_text() == seed,
        "no_commit": unchanged_head, "final_marker_seen": "OPENCODE_LINK_OK" in out,
        "swap_disabled": len(Path('/proc/swaps').read_text().splitlines()) == 1}
    if errors:
        report["errors"] = errors
    if process.returncode and err:
        report["stderr_excerpt"] = redact(err)[-500:]
    print(json.dumps(report), flush=True)
    if process.returncode or not verified or not unchanged_head or not report["swap_disabled"] or not report["final_marker_seen"] or not reasoning:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
