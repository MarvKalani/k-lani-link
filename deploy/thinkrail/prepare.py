"""Create new private test credentials and a disposable demo repository; never overwrite an existing run."""
import argparse
import json
import secrets
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def create_pair(pair, image="k-lani-link-host:local", concurrency=1, queued=0, managed=False):
    subprocess.run(["docker", "run", "--rm", "--network", "none", "--user", f"{os.getuid()}:{os.getgid()}",
        "-v", str(pair.parent) + ":/setup", image, "init", "--dir", "/setup/" + pair.name,
        "--signaling-url", "http://127.0.0.1:28788"], check=True)
    for node in ("host", "client"):
        path = pair / (node + ".toml")
        content = 'include_loopback_candidate = true\npairing_state_dir = "/var/lib/link"\n' + path.read_text()
        content = content.replace("request_timeout_seconds = 600", "request_timeout_seconds = 3600")
        if node == "host":
            content = 'upstream_socket = "/run/link/basalt.sock"\n' + content
            content = content.replace('upstream = "http://127.0.0.1:8080"', 'upstream = "http://localhost"')
        content = content.replace("max_concurrent_requests = 1", f"max_concurrent_requests = {concurrency}")
        content = content.replace("max_queued_requests = 0", f"max_queued_requests = {queued}")
        if managed:
            content = ('model_control_socket = "/run/link/control.sock"\n' if node == "host" else
                'management_token_file = "/run/secrets/management-token"\n') + content
        path.write_text(content)


def write_environment(target, model, preset, nvfp4=None):
    values = {"THINKRAIL_TEST_ROOT": str(target), "BASALT_MODEL_PATH": str(model), "BASALT_MODEL_PRESET": preset}
    if nvfp4:
        values["LINK_NVFP4_MODEL"] = str(nvfp4)
    if any(any(c in value for c in "\r\n'") for value in values.values()):
        raise ValueError("paths may not contain newlines or single quotes")
    (target / "compose.env").write_text("".join(key + "='" + value + "'\n" for key, value in values.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-file", type=Path, required=True)
    parser.add_argument("--root", type=Path, default=ROOT / ".thinkrail-test")
    parser.add_argument("--preset", choices=("coder", "nvfp4"), default="coder")
    parser.add_argument("--link-image", default="k-lani-link-host:local")
    parser.add_argument("--managed", action="store_true", help="Enable operator-only model switching over the existing tunnel")
    parser.add_argument("--nvfp4-model-file", type=Path)
    parser.add_argument("--parallel", type=int, choices=range(1, 9), default=2)
    args = parser.parse_args()
    if args.managed and not args.nvfp4_model_file:
        parser.error("--managed requires --nvfp4-model-file")
    nvfp4 = args.nvfp4_model_file.resolve(strict=True) if args.managed else None
    model = args.model_file.resolve(strict=True)
    if os.getuid() != 1000 or os.getgid() != 1000:
        parser.error("this Linux reference deployment uses UID/GID 1000; adapt the Compose users and volumes first")
    if not model.is_file():
        parser.error("model-file must name a local file")
    os.umask(0o077)
    target = args.root.resolve()
    target.mkdir(mode=0o700)
    (target / "recovery").mkdir(mode=0o700)
    (target / "metrics").mkdir(mode=0o700)
    create_pair(target / "pair", args.link_image,
        concurrency=args.parallel if args.managed else 1, queued=4 if args.managed else 0, managed=args.managed)
    if args.managed:
        (target / "management-token").write_text(secrets.token_urlsafe(32))
        (target / "managed.profile").write_text("enabled\n")
        models = [{"id":key,"path":path,"context_tokens":262144,"parallel":args.parallel,
            "slot_full_context":True,"prefill":512,"reasoning_budget_tokens":2048}
            for key,path in (("coder","/models/coder.basalt"),("nvfp4","/models/nvfp4.basalt"))]
        (target / "models.json").write_text(json.dumps({"default":args.preset,"models":models},indent=2)+"\n")
    demo = target / "work/demo"
    demo.mkdir(parents=True, mode=0o700)
    (demo / "README.md").write_text("# k-lani-link ThinkRail test\n\nSynthetic workspace for local Pi/Basalt testing.\n")
    (demo / "seed.txt").write_text("link-pi-256k\n")
    subprocess.run(["git", "init", "-b", "main", str(demo)], check=True, capture_output=True)
    subprocess.run(["git", "-C", str(demo), "add", "README.md", "seed.txt"], check=True)
    subprocess.run(["git", "-C", str(demo), "-c", "user.name=k-lani-link Test", "-c", "user.email=test@localhost",
        "-c", "commit.gpgSign=false", "-c", "core.hooksPath=/dev/null",
        "commit", "-m", "Create synthetic integration fixture"], check=True, capture_output=True)
    write_environment(target, model, args.preset, nvfp4)
    print("Private test setup created; credentials were not displayed.")


if __name__ == "__main__":
    main()
