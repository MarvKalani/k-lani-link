"""Exercise the installed Codex core using only a synthetic workspace. No transcripts are saved."""
import collections
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading

PROMPT = ("Read seed.txt using exec_command. Use apply_patch to create result.txt containing exactly the "
          "content of seed.txt followed by a second line 'verified' and a final newline. "
          "Read result.txt with exec_command to verify the change. Finish by saying link-agent-ok.")


def cli():
    run = subprocess.run(["codex", "exec", "--strict-config", "--json", "--ephemeral", "--skip-git-repo-check",
        "--sandbox", "danger-full-access", PROMPT], stdin=subprocess.DEVNULL, capture_output=True, text=True)
    events = [json.loads(line) for line in run.stdout.splitlines() if line.startswith("{")]
    assert run.returncode == 0, "Codex CLI failed: " + run.stderr[-1500:]
    assert any(e["type"] == "turn.completed" for e in events), "turn did not complete"
    items = [e["item"] for e in events if e["type"] == "item.completed"]
    assert any(i["type"] == "command_execution" and i.get("exit_code") == 0 for i in items)
    assert any(i["type"] == "file_change" and i.get("status") == "completed" for i in items)
    assert any(i["type"] == "agent_message" and "link-agent-ok" in i.get("text", "") for i in items)
    assert not any(i["type"] == "error" for i in items), "Codex used fallback metadata or reported an error"
    return {"mode": "cli", "completed_items": len(items)}


def app_server():
    process = subprocess.Popen(["codex", "app-server", "--stdio", "--strict-config"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
    errors = []
    threading.Thread(target=lambda: errors.append(process.stderr.read()), daemon=True).start()
    counts = collections.Counter()
    items = []
    pending = {}

    def send(method, params, identity=None):
        message = {"method": method, "params": params}
        if identity is not None:
            message["id"] = identity
        process.stdin.write(json.dumps(message) + "\n")
        process.stdin.flush()

    def read():
        line = process.stdout.readline()
        assert line, "app-server ended unexpectedly"
        event = json.loads(line)
        if "id" in event and "method" not in event:
            pending[event["id"]] = event
        if "method" in event:
            counts[event["method"]] += 1
            if event["method"] == "item/completed":
                items.append(event["params"]["item"])
            assert "id" not in event, "unexpected approval or external service request"
        return event

    def rpc(identity, method, params):
        send(method, params, identity)
        while identity not in pending:
            read()
        response = pending.pop(identity)
        assert "error" not in response, str(response.get("error"))
        return response["result"]

    try:
        rpc(1, "initialize", {"clientInfo": {"name": "link-check", "version": "1.0"},
                              "capabilities": {"experimentalApi": True}})
        send("initialized", {})
        models = rpc(2, "model/list", {"includeHidden": False})
        assert any(m.get("model") == os.environ["LINK_TEST_MODEL"] for m in models["data"]), "Basalt missing in model picker"
        thread = rpc(3, "thread/start", {"cwd": "/work", "modelProvider": "link",
                     "sandbox": "danger-full-access", "approvalPolicy": "never", "ephemeral": True})
        assert thread["modelProvider"] == "link"
        rpc(4, "turn/start", {"threadId": thread["thread"]["id"], "input": [{"type": "text", "text": PROMPT}]})
        while True:
            event = read()
            if event.get("method") == "turn/completed":
                assert event["params"]["turn"]["status"] == "completed", str(event["params"]["turn"].get("error"))
                break
        assert any(i["type"] == "commandExecution" and i.get("exitCode") == 0 for i in items)
        assert any(i["type"] == "fileChange" and i.get("status") == "completed" for i in items)
        assert any(i["type"] == "agentMessage" and "link-agent-ok" in i.get("text", "") for i in items)
        reasoning = sum(n for name, n in counts.items() if "reasoning" in name.lower() and "delta" in name.lower())
        assert reasoning > 0, "app-server did not stream reasoning"
        return {"mode": "app-server", "completed_items": len(items), "reasoning_deltas": reasoning}
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def main():
    assert {name for _, name in socket.if_nameindex()} == {"lo"}
    try:
        socket.create_connection(("1.1.1.1", 443), timeout=0.5)
    except OSError:
        pass
    else:
        raise AssertionError("unexpected internet route")
    result = app_server() if sys.argv[1] == "app-server" else cli()
    assert Path("result.txt").read_text() == Path("seed.txt").read_text() + "verified\n"
    print(json.dumps(result))


if __name__ == "__main__":
    main()
