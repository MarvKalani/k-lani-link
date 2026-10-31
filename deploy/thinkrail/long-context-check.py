"""Synthetic near-capacity recall test over the real k-lani-link API; retains no prompt or response file."""
import http.client
import json
from pathlib import Path
import socket
import time
import tomllib

MARKER = "KLANI_256K_OK_7429"


def connection():
    client = http.client.HTTPConnection("localhost", timeout=3600)
    client.sock = socket.socket(socket.AF_UNIX)
    client.sock.settimeout(3600)
    client.sock.connect("/run/link-api/api.sock")
    return client


def main():
    config = tomllib.loads(Path("/run/secrets/client.toml").read_text())
    headers = {"Authorization": "Bearer " + config["api_key"], "Content-Type": "application/json"}
    client = connection()
    client.request("GET", "/v1/models", headers=headers)
    models = json.loads(client.getresponse().read())
    client.close()
    model = models["data"][0]
    assert model["meta"]["n_ctx"] == 262144
    prompt = ("Remember this synthetic test marker for the final question: " + MARKER
              + ". The following text is irrelevant padding.\n" + "quaternion " * 260900
              + "\nQuestion: Return only the marker from the beginning of this message, without punctuation.")
    body = json.dumps({"model": model["id"], "stream": True, "max_tokens": 64,
        "reasoning_effort": "minimal", "temperature": 0,
        "stream_options": {"include_usage": True}, "messages": [{"role": "user", "content": prompt}]}).encode()
    print(json.dumps({"phase": "sending", "request_bytes": len(body), "context_capacity": 262144}), flush=True)
    start = time.monotonic()
    client = connection()
    client.request("POST", "/v1/chat/completions", body=body, headers=headers)
    response = client.getresponse()
    assert response.status == 200, response.read(500).decode(errors="replace")
    answer, usage, done = "", None, False
    while line := response.readline():
        if not line.startswith(b"data: "):
            continue
        data = line[6:].strip()
        if data == b"[DONE]":
            done = True
            break
        event = json.loads(data)
        if event.get("usage"):
            usage = event["usage"]
        for choice in event.get("choices", []):
            answer += choice.get("delta", {}).get("content", "") or ""
    client.close()
    assert done and usage, "stream did not complete with usage"
    assert 260000 <= usage["prompt_tokens"] < 262144
    result = {"phase": "completed", "input_tokens": usage["prompt_tokens"],
        "output_tokens": usage["completion_tokens"], "elapsed_seconds": round(time.monotonic() - start, 2),
        "request_bytes": len(body), "context_capacity": 262144, "marker_recalled": answer.strip() == MARKER}
    print(json.dumps(result), flush=True)
    assert result["marker_recalled"], "model did not recall the synthetic marker exactly"


if __name__ == "__main__":
    main()
