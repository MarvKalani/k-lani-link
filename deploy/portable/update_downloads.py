"""Refresh only the README download blocks from complete published Windows releases."""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import re
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
REPOSITORY = "MarvKalani/k-lani-link"
BASE = "https://github.com/" + REPOSITORY
START = "<!-- WINDOWS-DOWNLOADS:START -->"
END = "<!-- WINDOWS-DOWNLOADS:END -->"
SUFFIXES = ("windows-x64.exe", "windows-x64.zip", "windows-arm64.exe", "windows-arm64.zip")


def newest_complete(releases):
    candidates = []
    for release in releases:
        tag = release.get("tag_name", "")
        if release.get("draft") or not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?", tag):
            continue
        if not release.get("published_at"):
            continue
        version = tag[1:]
        expected = [f"k-lani-link-{version}-{suffix}" for suffix in SUFFIXES] + ["SHA256SUMS"]
        assets = {asset["name"]: asset for asset in release.get("assets", [])}
        if not all(name in assets and assets[name].get("state") == "uploaded"
                   and assets[name].get("size", 0) > 0 for name in expected):
            continue
        # Construct links ourselves; never insert supplied titles, bodies or arbitrary URLs.
        if not all(assets[name].get("browser_download_url") == f"{BASE}/releases/download/{tag}/{name}"
                   for name in expected):
            continue
        try:
            date = datetime.fromisoformat(release["published_at"].replace("Z", "+00:00"))
            if date.tzinfo is None:
                continue
        except (ValueError, TypeError):
            continue
        candidates.append((date, tuple(map(int, version.split("-", 1)[0].split("."))), release))
    return max(candidates, key=lambda item: item[:2])[2] if candidates else None


def block(release, german=False):
    heading = "## Windows herunterladen" if german else "## Download for Windows"
    source = BASE + "/archive/refs/heads/main.zip"
    if release is None:
        links = ("Noch kein vollständiges Windows-Paket veröffentlicht. " if german else
                 "No complete Windows package is currently published. ") + f"[GitHub Releases]({BASE}/releases).\n"
    else:
        version = release["tag_name"][1:]
        base = f"{BASE}/releases/download/v{version}"
        asset = lambda suffix: f"{base}/k-lani-link-{version}-{suffix}"
        if german:
            status = "Vorschau" if release.get("prerelease") else "Release"
            links = f"""**[⬇ Windows x64: EXE herunterladen]({asset('windows-x64.exe')})** · **[⬇ OpenCode + Docker: Setup-ZIP]({source})**

| Was möchtest du nutzen? | Download | Anleitung |
| --- | --- | --- |
| Portable EXE für Intel-/AMD-PCs | [Windows x64 als ZIP]({asset('windows-x64.zip')}) | [Native Einrichtung](deploy/portable/README.md#windows-connect-to-a-remote-model) |
| Windows auf ARM, z. B. Snapdragon | [ARM64 EXE]({asset('windows-arm64.exe')}) · [ARM64 ZIP]({asset('windows-arm64.zip')}) | [Native Einrichtung](deploy/portable/README.md#windows-connect-to-a-remote-model) |
| OpenCode im Browser, isoliert mit `D:\\OpenCode` | [Aktuelles Docker-Setup als ZIP]({source}) | [Docker-Desktop-Einrichtung](deploy/desktop/README.md) |

**Neueste veröffentlichte portable Version: {version} ({status}).**
[Versionshinweise]({BASE}/releases/tag/v{version}) · [Änderungen seit diesem Build]({BASE}/compare/v{version}...main) · [SHA-256-Prüfsummen]({base}/SHA256SUMS) · [macOS / Linux / alle Downloads]({BASE}/releases)
"""
        else:
            status = "preview" if release.get("prerelease") else "release"
            links = f"""**[⬇ Download Windows x64 EXE]({asset('windows-x64.exe')})** · **[⬇ OpenCode + Docker setup ZIP]({source})**

| What do you want to run? | Download | Instructions |
| --- | --- | --- |
| Portable executable on an Intel/AMD PC | [Windows x64 ZIP]({asset('windows-x64.zip')}) | [Native setup](deploy/portable/README.md#windows-connect-to-a-remote-model) |
| Windows on ARM, e.g. Snapdragon | [ARM64 EXE]({asset('windows-arm64.exe')}) · [ARM64 ZIP]({asset('windows-arm64.zip')}) | [Native setup](deploy/portable/README.md#windows-connect-to-a-remote-model) |
| OpenCode in the browser, isolated with `D:\\OpenCode` | [Current Docker setup ZIP]({source}) | [Docker Desktop setup](deploy/desktop/README.md) |

**Latest published portable version: {version} ({status}).**
[Release notes]({BASE}/releases/tag/v{version}) · [Changes since this build]({BASE}/compare/v{version}...main) · [SHA-256 checksums]({base}/SHA256SUMS) · [macOS / Linux / all downloads]({BASE}/releases)
"""
    note = ("Das Docker-ZIP enthält den aktuellen Quellstand samt Starter; die Images werden einmalig lokal gebaut. "
            "Die EXE enthält nur den Transport, keine OpenCode-Oberfläche; normale Quellcode-Pushes erneuern sie nicht. Der Windows-/Firmennetz-Test des Docker-Profils steht noch aus."
            if german else
            "The Docker ZIP contains the current source and launcher; build its images locally once. "
            "The EXE provides the transport only, without the OpenCode UI; source pushes do not rebuild published binaries. Docker Desktop / company-network acceptance is still pending.")
    if release is None:
        links += f"\n[OpenCode + Docker setup ZIP]({source}) · [Setup](deploy/desktop/README.md)\n"
    return f"{START}\n{heading}\n\n{links}\n{note}\n{END}"


def replace_block(text, replacement):
    if text.count(START) != 1 or text.count(END) != 1 or text.index(START) >= text.index(END):
        raise ValueError("README download markers are missing, duplicated or reversed")
    before, rest = text.split(START, 1)
    _, after = rest.split(END, 1)
    return before + replacement + after


def fetch_releases():
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "k-lani-link-download-links"}
    if os.environ.get("GITHUB_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    result = []
    for page in range(1, 101):
        request = urllib.request.Request(f"https://api.github.com/repos/{REPOSITORY}/releases?per_page=100&page={page}", headers=headers)
        with urllib.request.urlopen(request, timeout=30) as response:
            items = json.load(response)
        if not isinstance(items, list):
            raise ValueError("invalid releases response")
        result.extend(items)
        if len(items) < 100:
            return result
    raise ValueError("release pagination limit reached")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--releases-json", type=Path, help="Use saved API data for an offline check")
    args = parser.parse_args()
    releases = json.loads(args.releases_json.read_text()) if args.releases_json else fetch_releases()
    selected = newest_complete(releases)
    updates = {}
    for name, german in (("README.md", False), ("README.de.md", True)):
        path = ROOT / name
        updates[path] = replace_block(path.read_text(), block(selected, german))
    for path, text in updates.items():
        path.write_text(text)
    print("README Windows download links refreshed from published release assets.")


if __name__ == "__main__":
    main()
