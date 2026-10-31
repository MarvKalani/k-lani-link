"""Validate the completed fixed matrix and export aggregate evidence and decision tables.

Only use after run-matrix.py completes. No publication or production changes are performed.
"""
import argparse
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
MODES = ["mailbox", "stream", "post-stream", "sse", "longpoll", "h2-stream", "wss"]
SUITES = {"baseline-one": 14, "baseline-ten": 14, "upload-buffered": 7,
          "response-buffered": 7, "receiver-cut": 7, "idle-cut": 7, "large": 7}


def successful(row):
    return row["failed_tunnels"] == 0 and row["received_tokens"] == row["offered_tokens"]


def duration(value):
    if value is None:
        return "—"
    return f"{value / 1000:.2f} s" if value >= 1000 else f"{value:.0f} ms"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    assert manifest.get("finished_utc"), "Matrix is not complete."
    assert {run["suite"] for run in manifest["runs"]} == set(SUITES)
    assert len(manifest["runs"]) == len(SUITES)
    runs, by_suite = [], {}
    for run in manifest["runs"]:
        folder = Path(run["directory"])
        metadata = json.loads((folder / "metadata.json").read_text())
        rows = [json.loads(line) for line in (folder / "results.jsonl").read_text().splitlines()]
        suite = run["suite"]
        assert len(rows) == SUITES[suite], suite
        assert {row["mode"] for row in rows} == set(MODES), suite
        for row in rows:
            assert row["generation_seconds"] == 60
            assert row["requested_rtt_ms"] == 50
            assert row["synthetic_tokens_per_second_per_tunnel"] in [50, 100]
            assert row["tunnels"] in [1, 10]
            assert row["offered_tokens"] == 60 * row["tunnels"] * row["synthetic_tokens_per_second_per_tunnel"]
            assert 0 <= row["received_tokens"] <= row["offered_tokens"]
            assert row["failed_tunnels"] != 0 or row["received_tokens"] == row["offered_tokens"]
            if suite == "receiver-cut":
                assert row["injected_cuts"] == row["tunnels"], "Fault was not injected for every tunnel."
            if suite == "idle-cut":
                assert row["idle_seconds"] == 65
        by_suite[suite] = rows
        runs.append({"suite": suite, "metadata": metadata, "results": rows})
    bundle = {"schema": 2, "note": "Synthetic, volatile, one-direction carrier experiment; no production transport or durable endpoint resume.",
              "started_utc": manifest["started_utc"], "finished_utc": manifest["finished_utc"],
              "parallel_disjoint_cpu_sets": manifest["parallel_disjoint_cpu_sets"], "runs": runs}
    (HERE / "decision-results-2026-10-28.json").write_text(json.dumps(bundle, indent=2) + "\n")

    def row(suite, mode):
        return next(row for row in by_suite[suite] if row["mode"] == mode)

    def status(value, first=False):
        if successful(value):
            result = "Complete"
            if first:
                result += "; first " + duration(value["first_token_max_ms"])
            return result
        reason = ", ".join(sorted(value["failure_kinds"]))
        return f"Failed ({reason}); {value['received_tokens']:,}/{value['offered_tokens']:,} tokens"

    lines = ["# Seven-carrier decision matrix — 28 October 2026", "",
             "63 completed **test cases**, including intentionally failing cases. Every case uses 60 seconds of active synthetic generation; idle cases add 65 seconds of silence.", "",
             "These are local Docker measurements on a Ryzen 5 7600X, with small-VPS resource budgets, 50 ms shaped RTT and 500 Mbit/s egress. They are not EPYC, Internet, real-model, full-duplex or production-capacity measurements.", "",
             "[Full aggregate evidence](decision-results-2026-10-28.json) · [Method and reproduction](README.md)", "",
             "## Decision proposed for owner review", "",
             "Recommend the **acknowledged multi-record mailbox** as the first ordinary-HTTPS candidate. It completed every tested condition, with 96 ms median token delay versus roughly 71–72 ms for continuous readers at the selected 50 ms RTT. Do not implement production transport before owner review and the three state fixes.", "",
             "| Carrier | Proposed disposition |",
             "| --- | --- |",
             "| `mailbox` | Initial candidate: finite HTTP, acknowledged retention, all tested conditions complete |",
             "| `stream` | Defer: buffered upload postpones output; response tail/idle writer failures |",
             "| `post-stream` | Possible later optimization where response streaming is verified |",
             "| `sse` | Defer unless browser integration warrants its extra encoding |",
             "| `longpoll` | Reject destructive consumption; use the acknowledged mailbox rule |",
             "| `h2-stream` | Defer continuous upload; successful read-resume does not solve upload buffering/writer reconnect |",
             "| `wss` | Outside the ordinary-HTTP requirement; writer reconnect is also missing here |", "",
             "The [proposed delivery contract and three state tasks](../../docs/DESIGN-MAILBOX.md) are awaiting owner review. A later optimization is not authorization to build another production carrier.", "",
             "## Completion and failure behavior", "",
             "Baseline includes all four requested loads: 50/100 tokens/s per tunnel, with 1/10 tunnels. Proxy tests use ten tunnels at 100 tokens/s; the idle test uses one. The idle result includes the current prototype's **lack of writer reconnect** for continuous PUT/WSS, not an inherent limitation of those protocols.", "",
             "| Carrier | Baseline completed cases | Buffered upload | Buffered response | Reader cut / from=N | 60 s proxy idle cut | Larger budget |",
             "| --- | ---: | --- | --- | --- | --- | --- |"]
    for mode in MODES:
        baseline = [value for suite in ["baseline-one", "baseline-ten"] for value in by_suite[suite] if value["mode"] == mode]
        lines.append("| " + " | ".join([f"`{mode}`", f"{sum(map(successful,baseline))}/4",
                     status(row("upload-buffered",mode), True), status(row("response-buffered",mode), True),
                     status(row("receiver-cut",mode)), status(row("idle-cut",mode)), status(row("large",mode))]) + " |")
    lines += ["", "## Baseline: ten tunnels at 100 tokens/s each", "",
              "All rows offer 60,000 synthetic tokens. Latency starts at the scheduled synthetic token time and includes batching and delivery; first-token time also includes connection setup. Percentiles of a failed row describe only the delivered prefix.", "",
              "| Carrier | First token (slowest tunnel) | p50 | p99 | Relay CPU / client CPU (% of one CPU) | Relay / client peak RAM (MiB) | Relay egress (Mbit/s) |",
              "| --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    for mode in MODES:
        value = next(value for value in by_suite["baseline-ten"] if value["mode"] == mode and value["synthetic_tokens_per_second_per_tunnel"] == 100)
        lines.append(f"| `{mode}` | {duration(value['first_token_max_ms'])} | {duration(value['latency_p50_ms'])} | {duration(value['latency_p99_ms'])} | {100*value['relay_cpu_cores']:.2f}% / {100*value['client_cpu_cores']:.2f}% | {value['relay_memory_peak']/2**20:.1f} / {value['client_memory_peak']/2**20:.1f} | {value['relay_tx_bytes']*8/value['elapsed_seconds']/1e6:.2f} |")
    lines += ["", "## Receiver reconnection", "",
              "The receiver intentionally discards the next unverified record after 400 verified records, closes the receive transport and requests `?from=N` for that same record. Cryptographic keys and counters stay in memory. This is not process-crash recovery or a fresh handshake.", "",
              "| Carrier | Cuts injected | Reader reconnections | Longest reconnect-to-next-data | Outcome |",
              "| --- | ---: | ---: | ---: | --- |"]
    for mode in MODES:
        value = row("receiver-cut",mode)
        lines.append(f"| `{mode}` | {value['injected_cuts']} | {value['reader_reconnects']} | {duration(value['recovery_until_data_max_ms'])} | {status(value)} |")
    lines += ["", "## Interpretation limits", "",
              "- Completion requires the authenticated End record and all expected tokens. Receiving all tokens without a valid end is still a failure. Completion after a minute of buffering is not live streaming.",
              "- The mailbox is now non-destructive and acknowledges with the next cursor. The destructive `longpoll` remains a reference. The old single-record mailbox is not an eighth variant or the mailbox measured here.",
              "- Heartbeats are deliberately disabled in idle-cut cases. Continuous PUT and WSS writers do not implement outbound replay/reconnect. Reader resume is implemented and measured separately.",
              "- All seven modes use the same AEAD, 30 ms batching and 32-record retained window where applicable. Continuous readers ACK every eight records; mailbox ACKs via its next GET. Queue-full streaming uploads apply backpressure.",
              "- TLS certificates are verified. HTTP/2 negotiation is checked; Nginx terminates frontend HTTP/2 and proxies HTTP/1.1 upstream. No gRPC framework is measured.",
              "- Nginx HTTP body-buffering controls do not apply to an upgraded WebSocket pipe. WSS completing those rows is not evidence that a proxy forbidding Upgrade would allow it. HTTP/2 completed the selected response-buffering case; do not generalize either success or failure to every proxy.",
              "- CPU is cgroup CPU time divided by wall time; 100% means one CPU. Peak memory includes readiness/shaping helpers and page cache. Concurrent lanes use disjoint physical cores but share one host kernel and memory system.",
              "- No application request is replayed. No durable cursor, endpoint restart, complete hostile-relay test, corporate CONNECT/authentication proxy, real WAN or sustained capacity saturation is verified.", "",
              "## Validation", "",
              "Workspace formatting, Clippy with warnings denied, the final full workspace test run and the protocol WASM check passed. The experiment's four unit tests and Clippy passed; at that measurement revision the three state counterexamples still reproduced. The subsequent state-only fix replaces them with desired-behavior regression tests; the measured transport data is unchanged. The first workspace test attempt, restricted to spare CPU cores while benchmark networks were active, timed out at the existing five-second WebRTC setup limit in all four multiplex tests. A full unrestricted run after benchmark cleanup passed without changing production code or raising timeouts. That intermittent setup issue is not claimed fixed.", ""]
    (HERE / "DECISION.md").write_text("\n".join(lines))
    print("Validated and exported all 63 cases. Human review of the decision and contract is still required.")


if __name__ == "__main__":
    main()
