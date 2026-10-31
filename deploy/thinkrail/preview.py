"""Serve an exported static demo on its own loopback origin, without request logging or external resources."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import unquote, urlsplit

TYPES = {".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8", ".js": "text/javascript; charset=utf-8",
    ".webp": "image/webp", ".png": "image/png", ".svg": "image/svg+xml", ".ico": "image/x-icon",
    ".pdf": "application/pdf", ".docx": "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ".xlsx": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"}
CSP = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'none'; font-src 'self'; object-src 'none'; frame-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--port", type=int, default=24243)
    parser.add_argument("--entry", default="index.html")
    parser.add_argument("--allow-inline", action="store_true", help="Allow inline styles/scripts in trusted local demo exports; external connections stay blocked")
    args = parser.parse_args()
    root = args.directory.resolve(strict=True)
    entry = (root / args.entry).resolve(strict=True)
    if not entry.is_relative_to(root) or not entry.is_file() or entry.suffix not in TYPES:
        parser.error("entry must be a supported file inside the exported directory")
    csp = CSP.replace("script-src 'self'", "script-src 'self' 'unsafe-inline'").replace("style-src 'self'", "style-src 'self' 'unsafe-inline'") if args.allow_inline else CSP
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_HEAD(self):
            self.send_file(False)

        def do_GET(self):
            self.send_file(True)

        def send_file(self, body):
            if self.headers.get("Host") not in {f"127.0.0.1:{args.port}", f"localhost:{args.port}"}:
                self.send_error(403)
                return
            relative = unquote(urlsplit(self.path).path).lstrip("/") or args.entry
            try:
                path = (root / relative).resolve(strict=True)
                if not path.is_relative_to(root) or not path.is_file() or path.suffix not in TYPES or path.stat().st_size > 8 * 1024 * 1024:
                    raise ValueError("unavailable")
                content = path.read_bytes()
            except (OSError, ValueError):
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", TYPES[path.suffix])
            self.send_header("Content-Length", str(len(content)))
            self.send_header("Content-Security-Policy", csp)
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            if body:
                self.wfile.write(content)
    ThreadingHTTPServer(("127.0.0.1", args.port), Handler).serve_forever()


if __name__ == "__main__":
    main()
