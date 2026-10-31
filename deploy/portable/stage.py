"""Stage exactly one native executable and its public build metadata after CI tests."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {
    "windows-x64": "x86_64-pc-windows-msvc",
    "windows-arm64": "aarch64-pc-windows-msvc",
    "macos-x64": "x86_64-apple-darwin",
    "macos-arm64": "aarch64-apple-darwin",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=TARGETS, required=True)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    if TARGETS[args.platform] != args.target:
        parser.error("platform does not match target")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    suffix = ".exe" if args.platform.startswith("windows-") else ""
    binary = ROOT / "target" / args.target / "release" / ("k-lani-link" + suffix)
    result = subprocess.run([str(binary), "--version"], capture_output=True, check=True, text=True)
    if result.stdout.strip() != f"k-lani-link {version}" or result.stderr:
        raise RuntimeError("unexpected executable version")
    subprocess.run([str(binary), "--help"], stdout=subprocess.DEVNULL, check=True)
    # Running the built file on its native runner checks loader/runtime dependencies.
    if platform.system() == "Darwin":
        subprocess.run(["otool", "-L", str(binary)], check=True)
    if platform.system() == "Linux":
        subprocess.run(["ldd", str(binary)], check=True)
    output = ROOT / ".release" / "portable" / args.platform
    output.mkdir(parents=True, exist_ok=False)
    destination = output / ("k-lani-link" + suffix)
    shutil.copy2(binary, destination)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    metadata = {
        "name": "k-lani-link", "version": version, "platform": args.platform,
        "target": args.target, "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "binary": destination.name, "sha256": digest,
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "runner_os": platform.system(), "runner_architecture": platform.machine(),
        "workflow_run": os.environ.get("GITHUB_RUN_ID"),
        "publisher_signed": False, "apple_notarized": False,
        "validation": "Native Rust test suite including encrypted streaming with a synthetic backend; release executable starts. No real GPU/backend or cross-network test on this runner.",
    }
    (output / "BUILD.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    print(f"Staged {args.platform} {version}: SHA256 {digest}")


if __name__ == "__main__":
    main()
