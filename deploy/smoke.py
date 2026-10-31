"""Local Docker/WebRTC test with synthetic prompts; removes its own containers and credentials."""
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import tomllib
import uuid

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "target/release/k-lani-link"


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def main():
    with tempfile.TemporaryDirectory(prefix="link-container-test-") as directory:
        work = Path(directory)
        signal_port, api_port = port(), port()
        subprocess.run([str(BINARY), "init", "--dir", str(work / "pair"),
                        "--signaling-url", f"http://127.0.0.1:{signal_port}"], check=True)
        host_file, client_file = work / "pair/host.toml", work / "pair/client.toml"
        host_file.write_text(host_file.read_text().replace('upstream = "http://127.0.0.1:8080"',
            'upstream = "http://localhost"\nupstream_socket = "/run/link/basalt.sock"\npairing_state_dir = "/var/lib/link"'))
        client_file.write_text(client_file.read_text().replace('listen = "127.0.0.1:8787"', f'listen = "127.0.0.1:{api_port}"'))
        config = tomllib.loads(client_file.read_text())
        override = work / "mock.yaml"
        override.write_text('services:\n  basalt:\n    command: ["--mock-engine", "--script", "Checking the code.</think>\\n\\ncontainer-ok"]\n')
        env = {**os.environ, "LINK_HOST_CONFIG": str(host_file),
               "BASALT_MODEL_PATH": str(Path(os.environ["BASALT_MODEL_PATH"]).resolve(strict=True))}
        project = "link-test-" + uuid.uuid4().hex[:10]
        compose = ["docker", "compose", "-p", project, "-f", str(ROOT / "deploy/compose.yaml"), "-f", str(override)]
        children = []

        def run(args):
            return subprocess.run(args, env=env, capture_output=True, text=True, check=True, timeout=120).stdout

        def call(path, body=None):
            connection = http.client.HTTPConnection("127.0.0.1", api_port, timeout=10)
            connection.request("POST" if body is not None else "GET", path,
                json.dumps(body) if body is not None else None,
                {"Authorization": "Bearer " + config["api_key"], "Content-Type": "application/json"})
            response = connection.getresponse()
            payload = response.read()
            connection.close()
            return response.status, payload

        try:
            children.append(subprocess.Popen([str(BINARY), "signal", "--listen", f"127.0.0.1:{signal_port}",
                "--token-file", str(work / "pair/signaling-token.txt")], stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            run([*compose, "up", "-d", "--no-build"])
            basalt_id = run([*compose, "ps", "-q", "basalt"]).strip()
            inspection = json.loads(run(["docker", "inspect", basalt_id]))[0]
            assert inspection["HostConfig"]["NetworkMode"] == "none"
            assert inspection["HostConfig"]["ReadonlyRootfs"]
            assert inspection["HostConfig"]["LogConfig"]["Type"] == "none"
            assert not inspection["HostConfig"].get("PortBindings")
            assert any("gpu" in capability for item in inspection["HostConfig"]["DeviceRequests"] for capability in item["Capabilities"])
            run(["docker", "exec", basalt_id, "nvidia-smi", "--query-gpu=name", "--format=csv,noheader"])
            network_check = """import socket
assert {n for _, n in socket.if_nameindex()} == {'lo'}
try:
 socket.create_connection(('1.1.1.1',443),timeout=0.5)
except OSError:
 pass
else:
 raise AssertionError('unexpected outbound connectivity')
"""
            run(["docker", "exec", basalt_id, "/opt/basalt/.venv/bin/python", "-B", "-c", network_check])
            children.append(subprocess.Popen([str(BINARY), "client", "--config", str(client_file)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            deadline = time.monotonic() + 40
            while True:
                try:
                    status, raw = call("/health")
                    if status == 200 and json.loads(raw)["connected"]:
                        break
                except (OSError, http.client.HTTPException):
                    pass
                if time.monotonic() > deadline:
                    raise RuntimeError("container peer did not connect")
                time.sleep(0.2)
            children[0].terminate()
            children[0].wait(timeout=5)
            status, raw = call("/v1/models")
            assert status == 200
            model = json.loads(raw)["data"][0]["id"]
            status, raw = call("/v1/chat/completions", {"model": model, "stream": True,
                "messages": [{"role": "user", "content": "Synthetic connection test"}]})
            assert status == 200 and b'reasoning_content' in raw and b'[DONE]' in raw
            status, raw = call("/v1/responses", {"model": model, "stream": True, "input": "Synthetic connection test"})
            assert status == 200 and b'response.reasoning_text.delta' in raw and b'response.completed' in raw
            markers = list((client_file.parent / ".link-key-state").glob("*.used"))
            assert len(markers) == 1 and markers[0].read_bytes() == b""
            host_id = run([*compose, "ps", "-q", "link-host"]).strip()
            run(["docker", "exec", host_id, "python", "-B", "-c",
                 "from pathlib import Path; files=list(Path('/var/lib/link').glob('*.used')); assert len(files)==1 and files[0].read_bytes()==b''"])

            # Neither endpoint may establish another session using a consumed code.
            children[1].terminate()
            children[1].wait(timeout=5)
            children.append(subprocess.Popen([str(BINARY), "client", "--config", str(client_file)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE))
            deadline = time.monotonic() + 5
            while True:
                try:
                    status, raw = call("/health")
                    state = json.loads(raw)
                    if status == 200 and state["new_code_required"] and not state["connected"]:
                        break
                except (OSError, http.client.HTTPException):
                    pass
                if time.monotonic() > deadline:
                    raise RuntimeError("restarted client did not reject the consumed code")
                time.sleep(0.1)
            assert call("/v1/models")[0] == 503
            run([*compose, "restart", "link-host"])
            deadline = time.monotonic() + 5
            while True:
                state = json.loads(run(["docker", "inspect", host_id]))[0]["State"]
                if not state["Running"]:
                    assert state["ExitCode"] == 1
                    break
                if time.monotonic() > deadline:
                    raise RuntimeError("restarted host did not reject the consumed code")
                time.sleep(0.1)
            print("PASS: Docker network=none, GPU device, read-only root, log driver=none, Unix socket, real encrypted WebRTC, reasoning SSE and Responses after signaling shutdown; consumed code rejected on both endpoint restarts")
        finally:
            emitted = False
            for child in children:
                if child.poll() is None:
                    child.send_signal(signal.SIGTERM)
                try:
                    out, err = child.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill(); out, err = child.communicate()
                emitted = emitted or bool(out or err)
            subprocess.run([*compose, "down", "--volumes"], env=env, capture_output=True, timeout=90, check=False)
            if emitted:
                raise AssertionError("k-lani-link emitted application output")


if __name__ == "__main__":
    main()
