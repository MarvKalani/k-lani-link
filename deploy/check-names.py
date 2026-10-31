"""Fail when the former product name appears in a tracked or new file, a file name, an archive
member or a binary file.

The pattern is assembled from two halves so that this script does not match itself. Third-party
license texts are kept verbatim and are the only exception.
"""
import re
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORMER = re.compile("inter" + "com", re.IGNORECASE)
FORMER_BYTES = re.compile(("inter" + "com").encode(), re.IGNORECASE)
ALLOWED = ("THIRD_PARTY_LICENSES/",)


def places(name: str, data: bytes) -> list[str]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        text = None
    if text is not None:
        return [f"{name}:{number}" for number, line in enumerate(text.splitlines(), 1) if FORMER.search(line)]
    found = [f"{name} (binary)"] if FORMER_BYTES.search(data) else []
    if data[:4] == b"PK\x03\x04":
        try:
            with zipfile.ZipFile(ROOT / name) as archive:
                for member in archive.namelist():
                    if FORMER.search(member) or FORMER_BYTES.search(archive.read(member)):
                        found.append(f"{name} -> {member}")
        except (zipfile.BadZipFile, OSError):
            pass
    return found


def main():
    listing = subprocess.check_output(
        ["git", "-C", str(ROOT), "ls-files", "-z", "--cached", "--others", "--exclude-standard"])
    found = []
    for name in filter(None, listing.decode().split("\0")):
        if name.startswith(ALLOWED) or not (ROOT / name).is_file():
            continue
        if FORMER.search(name):
            found.append(name + " (file name)")
        try:
            found.extend(places(name, (ROOT / name).read_bytes()))
        except OSError:
            continue
    if found:
        print("The former product name must not reappear. Offending places:")
        print("\n".join("  " + place for place in found))
        return 1
    print("PASS: no trace of the former product name in files, file names, archives or binaries.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
