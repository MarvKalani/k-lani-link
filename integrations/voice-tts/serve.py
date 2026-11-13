"""Serve Audio8 and Ming voice clones through a local OpenAI-compatible endpoint."""

import argparse
import hmac
from http.client import HTTPException
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import sys
import threading
import time
from urllib.error import HTTPError
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener
import wave


CLONE_MODELS = ("audio8-tts-realistic-air", "ming-3.0-tts-0.5b")
SPEECH_PATH = "/v1/audio/speech"
MAX_BODY = 64 * 1024
WORKER_TIMEOUT = 480
INPUT_LIMIT = 4096
MAX_WAV = 64 * 1024 * 1024
CLONE_FIELDS = {"model", "input", "voice", "response_format"}
VOICE_MEMORY_MIB = 4096


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Handler(BaseHTTPRequestHandler):
    timeout = 30

    def log_message(self, format, *args):
        pass

    def authorized(self):
        if "Origin" in self.headers:
            return False
        if self.server.authorization is None:
            return self.headers.get("Host") in {"localhost", "127.0.0.1"}
        port = self.server.server_port
        return self.headers.get("Host") in {f"localhost:{port}", f"127.0.0.1:{port}"} and hmac.compare_digest(
            self.headers.get("Authorization", "").encode("latin-1"), self.server.authorization,
        )

    def respond(self, status, body, content_type="application/json"):
        if isinstance(body, dict):
            body = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        try:
            self.end_headers()
            self.wfile.write(body)
        except OSError:
            pass

    def failure(self, status, message):
        kind = "invalid_request_error" if status < 500 else "server_error"
        self.respond(status, {"error": {"message": message, "type": kind}})

    def do_GET(self):
        if not self.authorized():
            return self.failure(401, "invalid local API credential")
        if self.path != "/v1/models":
            return self.failure(404, "route not available")
        self.respond(200, {"object": "list", "data": [
            {"id": model, "object": "model", "owned_by": "local", "created": 0,
             "capability": "speech", "voices": ["local-reference"]}
            for model in CLONE_MODELS
        ]})

    def do_POST(self):
        if not self.authorized():
            return self.failure(401, "invalid local API credential")
        if self.path != SPEECH_PATH:
            return self.failure(404, "route not available")
        if self.headers.get_content_type() != "application/json":
            return self.failure(415, "application/json is required")
        try:
            size = int(self.headers.get("Content-Length", ""))
            if size < 1:
                raise ValueError
            if size > MAX_BODY:
                return self.failure(413, "speech request exceeds the size limit")
            raw = self.rfile.read(size)
            if len(raw) != size:
                raise ValueError
            data = json.loads(raw)
            if not isinstance(data, dict):
                raise ValueError
        except (ValueError, TimeoutError, RecursionError):
            return self.failure(400, "invalid speech request body")
        model = data.get("model", "magpietts" if self.server.nemo_authorization else None)
        if model == "magpietts" and self.server.nemo_authorization:
            return self.magpie(raw)
        if model not in CLONE_MODELS:
            return self.failure(404, "speech model not found")
        if data.keys() - CLONE_FIELDS:
            return self.failure(400, "unsupported fields for the configured voice")
        text = data.get("input")
        if not isinstance(text, str) or not text.strip():
            return self.failure(400, "input must contain text")
        text = text.strip()
        if len(text) > INPUT_LIMIT:
            return self.failure(400, f"input must contain 1 to {INPUT_LIMIT} characters")
        if data.get("voice", "local-reference") != "local-reference":
            return self.failure(400, "voice must be local-reference")
        if data.get("response_format", "wav") != "wav":
            return self.failure(400, "response_format must be wav")
        if not self.server.slot.acquire(blocking=False):
            return self.failure(429, "voice synthesis is busy")
        try:
            self.prepare_gpu()
            request = json.dumps({"model": model, "input": text}).encode()
            wav = subprocess.run(
                self.server.worker, input=request, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                timeout=WORKER_TIMEOUT, check=True, env=self.server.worker_env, cwd=self.server.state,
            ).stdout
            if not 44 <= len(wav) <= MAX_WAV or wav[:4] != b"RIFF" or wav[8:12] != b"WAVE":
                return self.failure(502, "voice engine returned invalid or oversized audio")
            self.respond(200, wav, "audio/wav")
        except subprocess.TimeoutExpired:
            self.failure(504, "voice synthesis timed out")
        except GpuBusy:
            self.failure(503, "GPU memory is busy. Free the model cache and try again")
        except (OSError, subprocess.CalledProcessError):
            self.failure(502, "voice synthesis failed")
        finally:
            self.server.slot.release()

    def prepare_gpu(self):
        if self.server.comfy_port is None and self.server.authorization is not None:
            return
        if gpu_free_memory() >= VOICE_MEMORY_MIB:
            return
        if self.server.comfy_port is None:
            raise GpuBusy
        base = f"http://127.0.0.1:{self.server.comfy_port}"
        try:
            with self.server.http.open(base + "/prompt", timeout=2) as response:
                pending = json.load(response)["exec_info"]["queue_remaining"]
            if pending:
                raise GpuBusy
            release = Request(
                base + "/free", data=b'{"unload_models":true,"free_memory":true}',
                headers={"Content-Type": "application/json"},
            )
            with self.server.http.open(release, timeout=2):
                pass
        except (OSError, HTTPException, KeyError, ValueError) as error:
            raise GpuBusy from error
        for _ in range(12):
            if gpu_free_memory() >= VOICE_MEMORY_MIB:
                return
            time.sleep(0.25)
        raise GpuBusy

    def magpie(self, body):
        request = Request(
            f"http://127.0.0.1:{self.server.nemo_port}{SPEECH_PATH}",
            data=body,
            headers={"Authorization": self.server.nemo_authorization, "Content-Type": "application/json"},
        )
        try:
            with self.server.http.open(request, timeout=120) as response:
                self.respond(response.status, response.read(), response.headers.get_content_type())
        except HTTPError as error:
            with error:
                self.respond(error.code, error.read(), error.headers.get_content_type())
        except (OSError, HTTPException):
            self.failure(502, "Magpie service is unavailable")


class GpuBusy(Exception):
    pass


def gpu_free_memory():
    command = ["nvidia-smi", "--id=0", "--query-gpu=memory.free", "--format=csv,noheader,nounits"]
    try:
        return int(subprocess.check_output(command, stderr=subprocess.DEVNULL, timeout=2))
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        raise GpuBusy from error


class UnixHTTPServer(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

    def server_bind(self):
        path = Path(self.server_address)
        parent = path.parent.stat()
        if path.parent.is_symlink() or parent.st_uid != os.geteuid() or parent.st_mode & 0o077:
            raise ValueError("the socket directory must be private")
        if path.is_socket() and not path.is_symlink():
            path.unlink()
        super().server_bind()
        path.chmod(0o600)


def serving(args, parser):
    if args.unix_socket is None:
        if not args.api_key_file or not args.nemo_key_file:
            parser.error("TCP mode requires --api-key-file and --nemo-key-file")
        return ThreadingHTTPServer, ("127.0.0.1", args.port)
    if not args.unix_socket.is_absolute() or args.api_key_file or args.nemo_key_file or args.comfy_port is not None:
        parser.error("use an absolute private socket path for a network-isolated container")
    if {name for _, name in socket.if_nameindex()} - {"lo"}:
        parser.error("Unix socket mode requires an isolated network namespace")
    return UnixHTTPServer, str(args.unix_socket)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8192)
    parser.add_argument("--nemo-port", type=int, default=8191)
    parser.add_argument("--comfy-port", type=int, help="release an idle ComfyUI GPU cache when memory is low")
    parser.add_argument("--api-key-file", type=Path)
    parser.add_argument("--nemo-key-file", type=Path)
    parser.add_argument("--unix-socket", type=Path)
    parser.add_argument("--voice-dir", required=True, type=Path)
    parser.add_argument("--model-config", type=Path)
    parser.add_argument("--state-dir", required=True, type=Path)
    parser.add_argument("--worker-python", required=True, type=Path)
    args = parser.parse_args()
    server_class, address = serving(args, parser)
    ports = [port for port in (args.port, args.nemo_port, args.comfy_port) if port is not None]
    if len(set(ports)) != len(ports) or any(not 1 <= port <= 65535 for port in ports):
        parser.error("use distinct loopback ports")

    os.umask(0o077)
    state = args.state_dir.resolve()
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    voice = args.voice_dir.resolve(strict=True)
    checkout = next(
        (parent for parent in Path(__file__).resolve().parents if (parent / ".git").exists()),
        None,
    )
    credentials = tuple(p for p in (args.api_key_file, args.nemo_key_file) if p is not None)
    if checkout and any(path.resolve().is_relative_to(checkout) for path in (voice, *credentials)):
        parser.error("keep the voice reference and keys outside the Git checkout")
    if state.stat().st_mode & 0o077 or voice.stat().st_mode & 0o077:
        parser.error("voice and state directories must be private")
    worker_script = Path(__file__).with_name("synthesize.py").resolve()
    # Give the worker the system libraries, model code, checkpoints, and
    # reference files without exposing the home directory or the API keys.
    model_config = (args.model_config or voice / "model.json").resolve(strict=True)
    files = json.loads(model_config.read_text(encoding="utf-8"))
    interpreter = args.worker_python.absolute()
    sources = [interpreter.parent.parent, interpreter.resolve(strict=True).parent.parent, worker_script.parent,
               model_config.parent] if args.unix_socket is None else []
    venv = interpreter.parent.parent / "pyvenv.cfg"
    if sources and venv.is_file():
        for line in venv.read_text().splitlines():
            name, _, value = line.partition("=")
            if name.strip() == "home":
                sources.append(Path(value.strip()).parent)
                break
    sources.extend(Path(files[name]).resolve(strict=True) for name in ("audio8_model", "ming_model", "ming_source"))
    if any(any(
        directory.is_relative_to(path) for directory in (state, voice, *credentials)
    ) for path in sources) or state.is_relative_to(voice) or voice.is_relative_to(state):
        parser.error("use separate directories for code, voice files, keys and the worker cache")
    for path in (voice / "reference.wav", voice / "reference.txt", *credentials):
        if not path.is_file() or path.is_symlink() or path.stat().st_mode & 0o077:
            parser.error("voice files and API key files must be private regular files")
    try:
        with wave.open(str(voice / "reference.wav")) as source:
            if not source.getnframes():
                raise ValueError
        if not (voice / "reference.txt").read_text(encoding="utf-8").strip():
            raise ValueError
    except (OSError, wave.Error, EOFError, ValueError):
        parser.error("configure a PCM WAV reference and its matching transcript")
    key, nemo_key = None, None
    worker = [str(interpreter), "-B", str(worker_script), str(voice), str(model_config)]
    if args.unix_socket is None:
        key, nemo_key = (p.read_bytes().strip() for p in credentials)
        if any(len(token) < 32 or not all(33 <= byte <= 126 for byte in token) for token in (key, nemo_key)):
            parser.error("configure a private token of at least 32 characters for each backend")
        sandbox = [
            "bwrap", "--unshare-net", "--unshare-pid", "--die-with-parent",
            "--dev-bind", "/dev", "/dev", "--proc", "/proc",
            "--tmpfs", "/tmp", "--tmpfs", "/dev/shm",
        ]
        system_dirs = [Path(path) for path in ("/usr", "/lib", "/lib64", "/bin", "/etc", "/sys")]
        for source in dict.fromkeys(system_dirs + sources):
            if source.exists():
                sandbox += ["--ro-bind", str(source.resolve(strict=True)), str(source)]
        worker = sandbox + [
            "--ro-bind", str(voice), str(voice), "--bind", str(state), str(state),
            "--chdir", str(state), "--", *worker,
        ]
    environment = {
        name: os.environ[name] for name in ("PATH", "LANG", "LC_ALL", "CUDA_VISIBLE_DEVICES")
        if name in os.environ
    }
    environment.update(
        HOME=str(state), XDG_CACHE_HOME=str(state / "cache"),
        HF_MODULES_CACHE=str(state / "hf_modules"), HF_HUB_OFFLINE="1",
        TRANSFORMERS_OFFLINE="1", HF_HUB_DISABLE_TELEMETRY="1",
        TOKENIZERS_PARALLELISM="false", OMP_NUM_THREADS="5",
    )
    if args.unix_socket is not None:
        environment["CUDA_CACHE_MAXSIZE"] = str(64 * 1024 * 1024)
    # Disable environment proxies for the loopback hop to NeMo.
    with server_class(address, Handler) as server:
        server.authorization = b"Bearer " + key if key else None
        server.nemo_authorization = "Bearer " + nemo_key.decode("ascii") if nemo_key else None
        server.nemo_port = args.nemo_port
        server.comfy_port = args.comfy_port
        server.http = build_opener(ProxyHandler({}), NoRedirect())
        server.worker = worker
        server.worker_env = environment
        server.state = state
        server.slot = threading.Lock()
        server.serve_forever()


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
