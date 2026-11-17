#!/usr/bin/env python3
"""Publish the seven tarballs prepared by npm-pack.sh, using npm's CI identity."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile
import time
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import Request, urlopen

REGISTRY = "https://registry.npmjs.org"
STARTER = "k-lani-link"


def package_set(directory, version):
    subprocess.run(["sha256sum", "--check", "--strict", "SHA256SUMS"], cwd=directory, check=True)
    packages = {}
    for file in sorted(directory.glob("*.tgz")):
        with tarfile.open(file, "r:gz") as archive:
            manifest = json.load(archive.extractfile("package/package.json"))
        name = manifest["name"]
        if manifest["version"] != version or name in packages:
            raise ValueError(f"Unexpected package or version in {file.name}")
        with file.open("rb") as content:
            integrity = "sha512-" + base64.b64encode(hashlib.file_digest(content, "sha512").digest()).decode()
        packages[name] = (file, manifest, integrity)

    dependencies = packages[STARTER][1]["optionalDependencies"]
    if len(dependencies) != 6 or set(packages) != {STARTER, *dependencies} or any(
        not name.startswith("@k-lani/link-") or dependency_version != version
        for name, dependency_version in dependencies.items()
    ):
        raise ValueError("Expected the starter and exactly six matching platform packages")
    return [(name, packages[name]) for name in [*sorted(dependencies), STARTER]]


def registry_integrity(name, version):
    package = quote(name, safe="@")
    request = Request(
        f"{REGISTRY}/{package}/{version}?verify={time.time_ns()}",
        headers={"Accept": "application/json", "Cache-Control": "no-cache"},
    )
    try:
        with urlopen(request, timeout=15) as response:
            return json.load(response)["dist"]["integrity"]
    except HTTPError as error:
        if error.code != 404:
            raise
        return None


def require_same(name, version, expected, actual):
    if expected != actual:
        raise ValueError(f"Registry integrity does not match the prepared {name}@{version}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("version")
    parser.add_argument("channel", choices=["latest", "next"])
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    version = args.version
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?", version):
        parser.error("expected a Link release version")
    if (args.channel == "next") != ("-" in version):
        parser.error("the npm channel must match the release version")
    packages = package_set(args.directory, version)
    if args.verify_only:
        for name, _ in packages:
            print(f"Prepared {name}@{version} for {args.channel}")
        return

    # Check the whole set before writing to npm. Retrying an interrupted release is safe
    # as long as the already published tarballs are byte-for-byte identical.
    existing = set()
    for name, (_, _, integrity) in packages:
        actual = registry_integrity(name, version)
        if actual is not None:
            require_same(name, version, integrity, actual)
            existing.add(name)

    for name, (file, _, integrity) in packages:
        if name in existing:
            print(f"Already published {name}@{version}", flush=True)
            continue
        command = ["npm", "publish", str(file.resolve()), "--access", "public", "--ignore-scripts",
                   "--registry", REGISTRY]
        # npm's default latest tag refuses to move below a newer stable version.
        if args.channel == "next":
            command.extend(["--tag", "next"])
        subprocess.run(command, check=True)
        # The first read can hit a cached 404 from before the upload.
        for _ in range(12):
            actual = registry_integrity(name, version)
            if actual is not None:
                require_same(name, version, integrity, actual)
                break
            time.sleep(5)
        else:
            raise RuntimeError(f"{name}@{version} did not appear in the registry")


if __name__ == "__main__":
    main()
