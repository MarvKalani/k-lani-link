"""Serve local MiniMax H3 workflows through the OpenAI Images and Videos APIs."""

import argparse
import base64
from copy import deepcopy
from email import policy
from email.parser import BytesParser
from http.client import HTTPException
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import secrets
import shutil
from threading import Lock
import time
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener
from uuid import UUID, uuid4


MODEL = "minimax-h3-image"
VIDEO_MODEL = "minimax-h3-video"
REQUEST_FIELDS = {"model", "prompt", "size", "n", "seed", "response_format", "output_format"}
VIDEO_FIELDS = {"model", "prompt", "size", "seconds", "seed", "input_reference"}
VIDEO_PATH = re.compile(r"/v1/videos/(video_[0-9a-f]{32})(/content)?")
MAX_REQUEST_BYTES = 64 * 1024
MAX_IMAGE_BYTES = 16 * 1024 * 1024
MAX_VIDEO_REQUEST_BYTES = 8 * 1024 * 1024
VIDEO_JOB_TTL = 3600
MAX_VIDEO_JOBS = 32
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


class ApiError(Exception):
    def __init__(self, status, message):
        super().__init__(message)
        self.status = status


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Comfy:
    def __init__(self, port, timeout, template):
        self.origin = f"http://127.0.0.1:{port}"
        self.timeout = timeout
        self.template = template
        self.http = build_opener(ProxyHandler({}), NoRedirect())
        self.running = Lock()

    def open(self, path, payload=None, timeout=10, content_type="application/json"):
        if isinstance(payload, dict):
            payload = json.dumps(payload).encode()
        headers = {"Content-Type": content_type} if payload is not None else {}
        request = Request(self.origin + path, data=payload, headers=headers)
        return self.http.open(request, timeout=timeout)

    def request(self, path, payload=None, limit=1024 * 1024, timeout=10, content_type="application/json"):
        with self.open(path, payload, timeout, content_type) as response:
            data = response.read(limit + 1)
        if len(data) > limit:
            raise ApiError(502, "ComfyUI response exceeds the size limit")
        return data

    def generate(self, prompt, width, height, seed):
        if not self.running.acquire(blocking=False):
            raise ApiError(429, "MiniMax H3 Image is busy")
        try:
            workflow = deepcopy(self.template)
            workflow["4"]["inputs"].update(prompt=prompt, width=width, height=height)
            workflow["6"]["inputs"]["seed"] = seed
            return self.render(workflow)
        finally:
            self.running.release()

    def render(self, workflow):
        deadline = time.monotonic() + self.timeout
        prompt_id = self.submit(workflow)
        while (remaining := deadline - time.monotonic()) > 0:
            history = json.loads(self.request(f"/history/{prompt_id}", timeout=min(10, remaining)))
            if prompt_id in history:
                result = history[prompt_id]
                if result["status"]["status_str"] != "success":
                    raise ApiError(502, "ComfyUI could not generate the image")
                image = result["outputs"]["8"]["images"][0]
                query = urlencode({
                    "filename": image["filename"],
                    "subfolder": image["subfolder"],
                    "type": image["type"],
                    # ComfyUI re-encodes RGB PNG here without the workflow metadata.
                    "channel": "rgb",
                })
                data = self.request(f"/view?{query}", limit=MAX_IMAGE_BYTES)
                if not data.startswith(PNG_SIGNATURE):
                    raise ApiError(502, "ComfyUI did not return a PNG")
                return data
            time.sleep(min(0.5, remaining))
        # Remove only this prompt if it is still queued. A running render finishes in ComfyUI.
        try:
            self.request("/queue", {"delete": [prompt_id]}, timeout=2)
        except (HTTPException, OSError):
            pass
        raise ApiError(504, "ComfyUI generation timed out")

    def submit(self, workflow, prompt_id=None):
        payload = {"prompt": workflow}
        if prompt_id is not None:
            payload["prompt_id"] = prompt_id
        try:
            queued = json.loads(self.request("/prompt", payload))
        except HTTPError as error:
            raise ApiError(502, "ComfyUI rejected the configured workflow") from error
        return str(UUID(queued["prompt_id"]))

    def upload_reference(self, job_id, image, extension):
        filename = f"{job_id}.{extension}"
        boundary = uuid4().hex
        mime = {"png": "image/png", "jpg": "image/jpeg"}[extension]
        prefix = (
            f'--{boundary}\r\nContent-Disposition: form-data; name="type"\r\n\r\ntemp\r\n'
            f'--{boundary}\r\nContent-Disposition: form-data; name="image"; filename="{filename}"\r\n'
            f"Content-Type: {mime}\r\n\r\n"
        ).encode("ascii")
        data = prefix + image + f"\r\n--{boundary}--\r\n".encode("ascii")
        uploaded = json.loads(self.request(
            "/upload/image", data, content_type=f"multipart/form-data; boundary={boundary}",
        ))
        if uploaded.get("name") != filename or uploaded.get("type") != "temp" or uploaded.get("subfolder"):
            raise ApiError(502, "ComfyUI did not accept the temporary reference image")
        return f"{filename} [temp]"


def video_request(content_type, body):
    if "\r" in content_type or "\n" in content_type:
        raise ApiError(400, "invalid multipart video upload")
    headers = f"Content-Type: {content_type}\r\nMIME-Version: 1.0\r\n\r\n".encode("ascii")
    form = BytesParser(policy=policy.default).parsebytes(headers + body)
    if not form.is_multipart() or form.defects:
        raise ApiError(400, "invalid multipart video upload")
    fields = {}
    for part in form.iter_parts():
        name = part.get_param("name", header="content-disposition")
        if part.get_content_disposition() != "form-data" or name not in VIDEO_FIELDS or name in fields:
            raise ApiError(400, "unsupported or duplicate video form field")
        value = part.get_payload(decode=True)
        if part.defects or value is None or part.get("Content-Transfer-Encoding") is not None:
            raise ApiError(400, "invalid video form field")
        if name == "input_reference":
            fields[name] = value
        else:
            if len(value) > MAX_REQUEST_BYTES:
                raise ApiError(400, "video text field is too large")
            fields[name] = value.decode("utf-8")
    if fields.get("model", VIDEO_MODEL) != VIDEO_MODEL:
        raise ApiError(404, "model not found")
    prompt = fields.get("prompt", "")
    if not prompt.strip():
        raise ApiError(400, "prompt must be a non-empty string")
    size = fields.get("size", "480x480")
    if not re.fullmatch(r"[0-9]{3,4}x[0-9]{3,4}", size):
        raise ApiError(400, "size must be WIDTHxHEIGHT")
    width, height = map(int, size.split("x"))
    if any(side < 256 or side > 1344 or side % 32 for side in (width, height)) or width * height > 1344 * 768:
        raise ApiError(400, "video dimensions must be multiples of 32 within the supported canvas")
    seconds = fields.get("seconds", "4")
    if seconds not in {"4", "8", "12"}:
        raise ApiError(400, "seconds must be 4, 8 or 12")
    seed = fields.get("seed")
    if seed is None:
        seed = secrets.randbits(64)
    elif not re.fullmatch(r"[0-9]{1,20}", seed) or (seed := int(seed)) >= 2**64:
        raise ApiError(400, "seed must be an unsigned 64-bit integer")
    image = fields.get("input_reference", b"")
    if image.startswith(PNG_SIGNATURE):
        extension = "png"
    elif image.startswith(b"\xff\xd8\xff"):
        extension = "jpg"
    else:
        raise ApiError(400, "input_reference must be an uploaded PNG or JPEG")
    return prompt, width, height, int(seconds), seed, image, extension


class VideoJobs:
    def __init__(self, comfy, template):
        self.comfy = comfy
        self.template = template
        self.lock = Lock()
        self.jobs = {}

    def create(self, prompt, width, height, seconds, seed, image, extension):
        prompt_id = uuid4()
        job_id = f"video_{prompt_id.hex}"
        now = int(time.time())
        job = {
            "id": job_id,
            "object": "video",
            "model": VIDEO_MODEL,
            "status": "queued",
            "progress": 0,
            "created_at": now,
            "expires_at": now + VIDEO_JOB_TTL,
            "size": f"{width}x{height}",
            "seconds": str(seconds),
        }
        with self.lock:
            self.jobs = {key: value for key, value in self.jobs.items() if value["expires_at"] > now}
            if len(self.jobs) >= MAX_VIDEO_JOBS:
                raise ApiError(429, "video job limit reached")
            self.jobs[job_id] = job
        try:
            workflow = deepcopy(self.template)
            workflow["5"]["inputs"]["image"] = self.comfy.upload_reference(job_id, image, extension)
            # H3's trained range starts near five seconds. Render enough frames for the trim.
            workflow["7"]["inputs"].update(prompt=prompt, width=width, height=height, length=max(124, seconds * 24))
            workflow["9"]["inputs"]["seed"] = seed
            workflow["13"]["inputs"]["trim"]["trim"]["duration"] = float(seconds)
            if self.comfy.submit(workflow, str(prompt_id)) != str(prompt_id):
                raise ApiError(502, "ComfyUI returned an unexpected video job ID")
        except Exception:
            with self.lock:
                self.jobs.pop(job_id, None)
            raise
        return job

    def inspect(self, job_id):
        with self.lock:
            saved = self.jobs.get(job_id)
            if saved is None or saved["expires_at"] <= time.time():
                raise ApiError(404, "video job not found or expired")
            job = saved.copy()
        prompt_id = UUID(job_id.removeprefix("video_"))
        try:
            details = json.loads(self.comfy.request(f"/api/jobs/{prompt_id}", limit=2 * 1024 * 1024))
        except HTTPError as error:
            if error.code == 404:
                raise ApiError(404, "video job is no longer available in ComfyUI") from error
            raise
        status = {
            "pending": "queued",
            "in_progress": "in_progress",
            "completed": "completed",
            "cancelled": "failed",
            "failed": "failed",
        }[details["status"]]
        job["status"] = status
        if status == "completed":
            job["progress"] = 100
            if finished := details.get("execution_end_time"):
                job["completed_at"] = finished // 1000
        elif status == "failed":
            job["error"] = {"code": "generation_failed", "message": "ComfyUI could not generate the video"}
        return job, details

    def open_content(self, job_id):
        job, details = self.inspect(job_id)
        if job["status"] != "completed":
            raise ApiError(409, "video is not ready for download")
        video = details["outputs"]["13"]["images"][0]
        filename = video["filename"]
        if video["type"] != "temp" or video["subfolder"] or not re.fullmatch(r"ComfyUI_temp_video_[0-9]+_\.mp4", filename):
            raise ApiError(502, "ComfyUI returned an unexpected video result")
        query = urlencode({"filename": filename, "type": "temp"})
        try:
            response = self.comfy.open(f"/view?{query}")
        except HTTPError as error:
            if error.code == 404:
                raise ApiError(404, "video file is no longer available in ComfyUI") from error
            raise
        if response.headers.get_content_type() != "video/mp4":
            response.close()
            raise ApiError(502, "ComfyUI did not return an MP4")
        return response


def image_request(body):
    if not isinstance(body, dict) or body.keys() - REQUEST_FIELDS:
        raise ApiError(400, "unsupported image request fields")
    if body.get("model", MODEL) != MODEL:
        raise ApiError(404, "model not found")
    prompt = body.get("prompt")
    if not isinstance(prompt, str) or not prompt.strip():
        raise ApiError(400, "prompt must be a non-empty string")
    count = body.get("n", 1)
    if type(count) is not int or count != 1:
        raise ApiError(400, "n must be 1")
    if body.get("response_format", "b64_json") != "b64_json":
        raise ApiError(400, "response_format must be b64_json")
    if body.get("output_format", "png") != "png":
        raise ApiError(400, "output_format must be png")
    size = body.get("size", "1024x1024")
    if size == "auto":
        size = "1024x1024"
    if not isinstance(size, str) or not re.fullmatch(r"[0-9]{3,4}x[0-9]{3,4}", size):
        raise ApiError(400, "size must be WIDTHxHEIGHT or auto")
    width, height = map(int, size.split("x"))
    if any(side < 256 or side > 2048 or side % 32 for side in (width, height)):
        raise ApiError(400, "size dimensions must be multiples of 32 from 256 through 2048")
    seed = body.get("seed")
    if seed is None:
        seed = secrets.randbits(64)
    elif type(seed) is not int or not 0 <= seed < 2**64:
        raise ApiError(400, "seed must be an unsigned 64-bit integer")
    return prompt, width, height, seed


class Handler(BaseHTTPRequestHandler):
    timeout = 10

    def log_message(self, format, *args):
        pass

    def respond(self, status, payload):
        body = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        try:
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def failure(self, status, message):
        error_type = "invalid_request_error" if status < 500 else "server_error"
        self.respond(status, {"error": {"message": message, "type": error_type}})

    def local_request(self):
        port = self.server.server_port
        return "Origin" not in self.headers and self.headers.get("Host") in {
            f"127.0.0.1:{port}", f"localhost:{port}",
        }

    def do_GET(self):
        if not self.local_request():
            return self.failure(403, "local requests only")
        if self.path == "/v1/models":
            return self.respond(200, {
                "object": "list",
                "data": [
                    {"id": model, "object": "model", "created": 0, "owned_by": "local"}
                    for model in (MODEL, VIDEO_MODEL)
                ],
            })
        video = VIDEO_PATH.fullmatch(self.path)
        if video is None:
            return self.failure(404, "route not available")
        job_id, content = video.groups()
        try:
            if not content:
                job, _ = self.server.videos.inspect(job_id)
                return self.respond(200, job)
            upstream = self.server.videos.open_content(job_id)
        except ApiError as error:
            return self.failure(error.status, str(error))
        except (HTTPException, OSError, ValueError, KeyError, TypeError, IndexError, AttributeError):
            return self.failure(502, "ComfyUI is unavailable or returned an invalid video result")
        with upstream:
            self.send_response(200)
            self.send_header("Content-Type", "video/mp4")
            self.send_header("Cache-Control", "no-store")
            length = upstream.headers.get("Content-Length", "")
            if length.isascii() and length.isdecimal():
                self.send_header("Content-Length", length)
            try:
                self.end_headers()
                shutil.copyfileobj(upstream, self.wfile, length=64 * 1024)
            except (HTTPException, OSError):
                pass

    def do_POST(self):
        if not self.local_request():
            return self.failure(403, "local requests only")
        video = self.path == "/v1/videos"
        if not video and self.path != "/v1/images/generations":
            return self.failure(404, "route not available")
        media_type = "multipart/form-data" if video else "application/json"
        if self.headers.get_content_type() != media_type:
            return self.failure(415, f"{media_type} is required")
        try:
            length = int(self.headers.get("Content-Length", ""))
            if length < 1:
                raise ValueError
            limit = MAX_VIDEO_REQUEST_BYTES if video else MAX_REQUEST_BYTES
            if length > limit:
                return self.failure(413, "media request exceeds the size limit")
            body = self.rfile.read(length)
            if len(body) != length:
                raise ValueError
            args = video_request(self.headers["Content-Type"], body) if video else image_request(json.loads(body))
        except ApiError as error:
            return self.failure(error.status, str(error))
        except (ValueError, TimeoutError):
            return self.failure(400, "invalid request body")
        try:
            if video:
                return self.respond(200, self.server.videos.create(*args))
            data = self.server.comfy.generate(*args)
        except ApiError as error:
            return self.failure(error.status, str(error))
        except (HTTPException, OSError, ValueError, KeyError, TypeError, IndexError, AttributeError):
            return self.failure(502, "ComfyUI is unavailable or returned an invalid result")
        self.respond(200, {
            "created": int(time.time()),
            "data": [{"b64_json": base64.b64encode(data).decode("ascii")}],
        })


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8190)
    parser.add_argument("--comfy-port", type=int, default=8188)
    parser.add_argument("--timeout", type=int, default=540, help="render deadline in seconds")
    args = parser.parse_args()
    if not (1 <= args.port <= 65535 and 1 <= args.comfy_port <= 65535):
        parser.error("ports must be between 1 and 65535")
    if args.port == args.comfy_port or args.timeout < 1:
        parser.error("use distinct ports and a positive timeout")
    image = json.loads(Path(__file__).with_name("minimax-h3-image.json").read_text(encoding="utf-8"))
    video = json.loads(Path(__file__).with_name("minimax-h3-video.json").read_text(encoding="utf-8"))
    with ThreadingHTTPServer(("127.0.0.1", args.port), Handler) as server:
        server.comfy = Comfy(args.comfy_port, args.timeout, image)
        server.videos = VideoJobs(server.comfy, video)
        print(f"Serving {MODEL} and {VIDEO_MODEL} on http://127.0.0.1:{args.port}/v1", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
