"""Download and verify the pinned official Linux x64 OpenCode archive at build time."""
import hashlib
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import urllib.request
import shutil


def main():
    release = json.loads(Path(__file__).with_name("release.json").read_text())
    root = Path(sys.argv[1])
    root.mkdir(parents=True, exist_ok=True)
    target = root / "opencode"
    if target.exists():
        raise FileExistsError("destination already exists")
    with tempfile.TemporaryFile() as archive:
        digest = hashlib.sha256()
        size = 0
        with urllib.request.urlopen(release["url"], timeout=120) as response:
            while chunk := response.read(1024 * 1024):
                digest.update(chunk)
                size += len(chunk)
                archive.write(chunk)
        if digest.hexdigest() != release["sha256"] or size != release["size_bytes"]:
            raise ValueError("OpenCode release integrity check failed")
        archive.seek(0)
        with tarfile.open(fileobj=archive, mode="r:gz") as tar:
            candidates = [member for member in tar.getmembers() if member.isfile() and Path(member.name).name == "opencode"]
            if len(candidates) != 1:
                raise ValueError("archive must contain exactly one OpenCode executable")
            with tar.extractfile(candidates[0]) as source, target.open("xb") as destination:
                shutil.copyfileobj(source, destination)
    target.chmod(0o755)
    (root / "installation.json").write_text(json.dumps({**release, "archive_verified": True}, indent=2))


if __name__ == "__main__":
    main()
