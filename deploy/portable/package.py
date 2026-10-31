"""Verify CI artifacts, then package only executables, public metadata and notices."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import urljoin
import zipfile

from stage import ROOT, TARGETS


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    args = parser.parse_args()
    commit = subprocess.check_output(["git", "rev-parse", args.commit + "^{commit}"], cwd=ROOT, text=True).strip()
    if commit != subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip():
        parser.error("check out the exact build commit before packaging")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?", version):
        parser.error("unexpected version")
    notice_file = args.artifacts / "portable-notices" / "RUST-LICENSES.txt"
    notices = notice_file.read_bytes()
    if len(notices) < 1000:
        raise ValueError("missing dependency license texts")
    base = f"https://github.com/MarvKalani/k-lani-link/blob/{commit}/deploy/portable/"
    guide = (ROOT / "deploy" / "portable" / "README.md").read_text()
    guide = re.sub(r"\]\((\.\.?/[^)]+)\)", lambda match: "](" + urljoin(base, match[1]) + ")", guide)
    common = {name: (ROOT / name).read_bytes() for name in ("LICENSE", "LICENSING.md", "COMMERCIAL-LICENSE.md")}
    common.update({"GETTING-STARTED.md": guide.encode(), "RUST-LICENSES.txt": notices})
    prepared = []
    for platform, target in TARGETS.items():
        directory = args.artifacts / ("portable-" + platform)
        record = json.loads((directory / "BUILD.json").read_text())
        name = "k-lani-link.exe" if platform.startswith("windows-") else "k-lani-link"
        if any(record.get(key) != expected for key, expected in {
            "version": version, "commit": commit, "platform": platform,
            "target": target, "binary": name, "publisher_signed": False, "apple_notarized": False,
        }.items()):
            raise ValueError("mismatched build record for " + platform)
        binary = directory / name
        if binary.is_symlink() or not binary.is_file():
            raise ValueError("invalid binary artifact")
        data = binary.read_bytes()
        if hashlib.sha256(data).hexdigest() != record["sha256"]:
            raise ValueError("binary checksum mismatch for " + platform)
        prepared.append((platform, name, data, (directory / "BUILD.json").read_bytes()))
    args.output.mkdir(parents=True, exist_ok=False)
    for platform, name, data, metadata in prepared:
        stem = f"k-lani-link-{version}-{platform}"
        with zipfile.ZipFile(args.output / (stem + ".zip"), "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for member, content in {name: data, "BUILD.json": metadata, **common}.items():
                entry = zipfile.ZipInfo(stem + "/" + member, date_time=(1980, 1, 1, 0, 0, 0))
                entry.create_system = 3
                entry.external_attr = (0o100755 if member == name else 0o100644) << 16
                entry.compress_type = zipfile.ZIP_DEFLATED
                archive.writestr(entry, content)
        if platform.startswith("windows-"):
            (args.output / (stem + ".exe")).write_bytes(data)
    (args.output / "RUST-LICENSES.txt").write_bytes(notices)
    sums = "".join(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + path.name + "\n" for path in sorted(args.output.iterdir()))
    (args.output / "SHA256SUMS").write_text(sums, encoding="ascii")
    print(f"Packaged {len(prepared)} verified platforms from {commit}")


if __name__ == "__main__":
    main()
