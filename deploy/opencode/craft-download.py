"""Fetch pinned official Craft releases; retain the CLIs and all bundled documentation/license notices."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile
import urllib.request


def main():
    root = Path(sys.argv[1])
    (root / "bin").mkdir(parents=True)
    pins = json.loads(Path(__file__).with_name("craft-release.json").read_text())
    for pin in pins:
        with tempfile.TemporaryFile() as archive:
            digest, size = hashlib.sha256(), 0
            with urllib.request.urlopen(pin["url"], timeout=120) as source:
                while chunk := source.read(1024 * 1024):
                    digest.update(chunk); archive.write(chunk); size += len(chunk)
            if digest.hexdigest() != pin["sha256"] or size != pin["size_bytes"]:
                raise ValueError("Craft release integrity check failed")
            archive.seek(0)
            prefix = pin["name"] + "-" + pin["version"] + "-linux-x86_64/"
            executable = "bin/" + pin["name"] + "-cli"
            found = False
            with tarfile.open(fileobj=archive, mode="r:gz") as tar:
                for member in tar.getmembers():
                    if not member.isfile() or not member.name.startswith(prefix):
                        continue
                    relative = Path(member.name.removeprefix(prefix))
                    if relative.is_absolute() or ".." in relative.parts:
                        raise ValueError("invalid release path")
                    if str(relative) != executable and not str(relative).startswith("share/doc/" + pin["name"] + "/"):
                        continue
                    destination = root / relative
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    with tar.extractfile(member) as source, destination.open("xb") as output:
                        shutil.copyfileobj(source, output)
                    if str(relative) == executable:
                        destination.chmod(0o755); found = True
            if not found:
                raise ValueError("CLI missing from release")
    (root / "installation.json").write_text(json.dumps({"releases": pins, "archives_verified": True}, indent=2))


if __name__ == "__main__":
    main()
