"""Generate Rust redistribution notices with a checksum-pinned build-time tool."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
URL = "https://github.com/EmbarkStudios/cargo-about/releases/download/0.9.3/cargo-about-0.9.3-x86_64-unknown-linux-musl.tar.gz"
SHA256 = "3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    tools = ROOT / ".release" / "portable-tools"
    tools.mkdir(parents=True, exist_ok=True)
    with urllib.request.urlopen(URL, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != SHA256:
        raise RuntimeError("cargo-about download checksum mismatch")
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        entries = [entry for entry in archive.getmembers() if entry.isfile() and Path(entry.name).name == "cargo-about"]
        if len(entries) != 1:
            raise RuntimeError("unexpected cargo-about archive")
        executable = tools / "cargo-about"
        executable.write_bytes(archive.extractfile(entries[0]).read())
        executable.chmod(0o755)
    raw = subprocess.check_output([
        str(executable), "generate", "--locked", "--fail", "--format", "json",
        "--config", str(ROOT / "deploy" / "portable" / "about.toml"),
    ], cwd=ROOT)
    report = json.loads(raw)
    (output / "RUST-LICENSES.json").write_bytes(raw)
    lines = ["k-lani-link Rust dependency notices", "Generated with cargo-about 0.9.3 from Cargo.lock.", ""]
    for license in report["licenses"]:
        lines.extend([license["name"], "=" * 72])
        for usage in license["used_by"]:
            crate = usage["crate"]
            lines.append(f"{crate['name']} {crate['version']}")
        lines.extend(["", license["text"], ""])
    (output / "RUST-LICENSES.txt").write_text("\n".join(lines), encoding="utf-8")
    print(f"Generated {len(report['licenses'])} license sections")


if __name__ == "__main__":
    main()
