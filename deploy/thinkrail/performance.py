"""Validate the local backend's numeric snapshot before publishing it to the dashboard."""
import math

NUMBERS = {"context_tokens_per_slot", "slots", "active_requests", "generated_tokens", "decode_window_seconds",
           "live_decode_tokens_per_second", "live_prefill_tokens_per_second", "observed_at", "process_started_at"}
LAST = {"finished_at", "output_tokens", "prefill_tokens", "cached_tokens", "prefill_ms",
        "prefill_tokens_per_second", "decode_ms", "decode_tokens_per_second", "ttft_ms"}


def number(value):
    return value if not isinstance(value, bool) and isinstance(value, (int, float)) and math.isfinite(value) and value >= 0 else None


def sanitize(raw, now, container_started_at):
    unavailable = {"available": False, "reason": "numeric exporter unavailable or stale"}
    if not isinstance(raw, dict) or raw.get("schema") != 1 or raw.get("backend") != "basalt":
        return unavailable
    observed, started = number(raw.get("observed_at")), number(raw.get("process_started_at"))
    if observed is None or started is None or not -2 <= now - observed <= 5 or started < container_started_at - 2:
        return unavailable
    state = raw.get("state")
    if state not in {"reading", "generating", "idle", "unloaded"}:
        return unavailable
    out = {"available": True, "backend": "basalt", "state": state,
           "model": str(raw.get("model", ""))[:200], **{k: number(raw.get(k)) for k in NUMBERS}}
    last = raw.get("last")
    out["last"] = {k: number(last.get(k)) for k in LAST} if isinstance(last, dict) else None
    return out
