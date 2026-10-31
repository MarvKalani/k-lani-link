"""Prepare a private second-client fixture; reuse the existing k-lani-link tunnel and Basalt backend."""
import argparse
import base64
import os
from pathlib import Path
import secrets
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT / ".opencode-test")
    parser.add_argument("--link-root", type=Path, default=ROOT / ".thinkrail-test")
    parser.add_argument("--client-config", type=Path, help="Use an existing remote-client configuration instead of the local lab")
    parser.add_argument("--api-volume", default="k-lani-thinkrail-test_api_socket")
    args = parser.parse_args()
    if os.getuid() != 1000 or os.getgid() != 1000:
        parser.error("adapt the reference deployment for your UID/GID first")
    client = (args.client_config or args.link_root / "pair/client.toml").resolve(strict=True)
    os.umask(0o077)
    root = args.root.resolve(); root.mkdir(mode=0o700)
    for name in ("secrets", "ui", "metrics", "work"):
        (root / name).mkdir(mode=0o700)
    password = secrets.token_urlsafe(32)
    (root / "secrets/server-password.txt").write_text(password + "\n")
    auth = base64.b64encode(("opencode:" + password).encode()).decode()
    (root / "secrets/proxy-auth.conf").write_text('proxy_set_header Authorization "Basic ' + auth + '";\n')
    (root / "work/seed.txt").write_text("link-opencode-256k\n")
    (root / "work/README.md").write_text("# OpenCode k-lani-link lab\n\nSynthetic files for a local coding-agent test.\n")
    subprocess.run(["git", "init", "-b", "main", str(root / "work")], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(root / "work"), "add", "seed.txt", "README.md"], check=True)
    subprocess.run(["git", "-C", str(root / "work"), "-c", "user.name=k-lani-link Test", "-c", "user.email=test@localhost",
        "-c", "commit.gpgSign=false", "-c", "core.hooksPath=/dev/null", "commit", "-m", "Create synthetic OpenCode fixture"],
        check=True, capture_output=True)
    for value in (str(root), str(client), args.api_volume):
        if any(c in value for c in "\r\n'"):
            raise ValueError("unsupported path characters")
    (root / "compose.env").write_text("OPENCODE_TEST_ROOT='" + str(root) + "'\nLINK_CLIENT_CONFIG='" + str(client) + "'\nLINK_API_VOLUME='" + args.api_volume + "'\n")
    print("Private OpenCode test setup created; no credentials displayed.")


if __name__ == "__main__":
    main()
