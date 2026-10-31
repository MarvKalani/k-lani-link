"""Build-time-only download of an official, pinned ThinkRail release."""
import hashlib
import json
from pathlib import Path
import sys
import urllib.request

RELEASE = json.loads(Path(__file__).with_name("release.json").read_text())
VERSION, SHA256, URL = RELEASE["version"], RELEASE["sha256"], RELEASE["url"]


def main():
    root = Path(sys.argv[1])
    binary = root / "bin/thinkrail"
    binary.parent.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256()
    if binary.exists():
        raise FileExistsError("download destination already exists")
    try:
        with urllib.request.urlopen(URL, timeout=120) as source, binary.open("xb") as target:
            while chunk := source.read(1024 * 1024):
                digest.update(chunk)
                target.write(chunk)
        if digest.hexdigest() != SHA256:
            raise ValueError("ThinkRail release checksum mismatch")
        binary.chmod(0o755)
        (root / "installation.json").write_text(json.dumps({"product": "ThinkRail", "version": VERSION,
            "sha256": SHA256, "channel": RELEASE["channel"], "commit": RELEASE["commit"],
            "pi_version": RELEASE["pi_version"], "installer": "verified official release download", "source": URL}, indent=2))
    except BaseException:
        binary.unlink(missing_ok=True)
        raise


if __name__ == "__main__":
    main()
