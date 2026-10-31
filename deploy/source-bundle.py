"""Create a reviewable source bundle; never publishes, includes runtime state or assigns a license."""
import gzip
import hashlib
import io
from pathlib import Path
import re
import tarfile

ROOT = Path(__file__).resolve().parents[1]
TREES = ("src", "crates", "tests", "examples", "integrations", "deploy", "assets", "website", "THIRD_PARTY_LICENSES", ".github", "docs", "legal")
FILES = ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md", "README.de.md", "REFERENCE.md", "SECURITY.md", "THIRD_PARTY.md",
    "LICENSE", "COMMERCIAL-LICENSE.md", "LICENSING.md", "QUICKSTART.md", "CHANGELOG.md", ".gitignore", ".dockerignore", ".gitattributes")
SUFFIXES = {".rs", ".toml", ".lock", ".md", ".py", ".sh", ".ps1", ".json", ".yaml", ".yml", ".txt", ".patch", ".html", ".css", ".js", ".conf", ".webp"}
SPECIAL = {"Dockerfile", "guard.Dockerfile", "link.Dockerfile", "basalt.Dockerfile", "ui.Dockerfile", "LICENSE", "BASE_COMMIT", ".env.example", "client.env.example"}
# Only these reviewed synthetic documents are public binaries inside the archive.
DEMO_BINARIES = {"website/examples/invoice.docx", "website/examples/invoice.pdf", "website/examples/ledger.xlsx"}
# The reviewed PHP relay sources, including its two access-denying .htaccess files.
PHP_RELAY = {"deploy/php-relay/" + name for name in (
    "k-lani-relay.php", "test_relay.php", "wordpress/k-lani-link-relay.php", "wordpress/relay.php",
    "wordpress/uninstall.php", "standalone/index.php", "standalone/sweep.php", "standalone/config.example.php",
    "standalone/.htaccess", "standalone/data/.htaccess")}
# The shareable demo clips live in the repository and the release page; as part of the
# archive they would add large binary previews; distribute them separately instead.
LEFT_OUT = ("assets", "demo")


def main():
    paths = [ROOT / name for name in FILES]
    for name in TREES:
        paths.extend(p for p in (ROOT / name).rglob("*") if p.is_file())
    selected = {}
    left_out = 0
    for path in sorted(paths):
        relative = path.relative_to(ROOT)
        if relative.parts[:2] == LEFT_OUT and relative.suffix == ".gif":
            left_out += 1
            continue
        if path.name in {".env", "client.env"} or path.name.endswith(".local.toml"):
            continue
        if path.name in {"auth.json", "mcp-auth.json", "nodes.json", "host.toml", "client.toml", "pair-code.txt", "signaling-token.txt"}:
            raise ValueError("private runtime file inside source tree: " + str(relative))
        # A PHP relay's real config.php (token hashes) and its stored ciphertext stay private.
        if relative.parts[:2] == ("deploy", "php-relay") and str(relative) not in PHP_RELAY and (
                path.suffix == ".php" or path.name.startswith(".") or "data" in relative.parts):
            raise ValueError("private PHP relay file inside source tree: " + str(relative))
        if "__pycache__" in relative.parts or path.suffix == ".pyc":
            continue
        if path.is_symlink() or str(relative) not in PHP_RELAY and any(part.startswith(".") for part in relative.parts if part not in {".github", ".env.example", ".gitignore", ".dockerignore", ".gitattributes"}):
            raise ValueError("unexpected private or linked file in source tree: " + str(relative))
        if (path.suffix not in SUFFIXES and path.name not in SPECIAL and str(relative) not in FILES
                and str(relative) not in DEMO_BINARIES and str(relative) not in PHP_RELAY):
            raise ValueError("unreviewed file type: " + str(relative))
        content = path.read_bytes()
        if len(content) > 8 * 1024 * 1024:
            raise ValueError("unexpected large source file: " + str(relative))
        if path.suffix != ".webp" and str(relative) not in DEMO_BINARIES:
            text = content.decode("utf-8")
            if re.search(r"-----BEGIN (?:RSA |OPENSSH )?PRIVATE KEY-----|sk-proj-[A-Za-z0-9_-]{20,}", text):
                raise ValueError("potential credential in " + str(relative))
        selected[str(relative)] = content
    output = ROOT / ".release"
    output.mkdir(exist_ok=True, mode=0o700)
    manifest = "".join(hashlib.sha256(data).hexdigest() + "  " + name + "\n" for name, data in selected.items()).encode()
    selected["SOURCE-SHA256SUMS"] = manifest
    archive_path = output / "k-lani-link-source.tar.gz"
    with archive_path.open("wb") as destination, gzip.GzipFile(filename="", mode="wb", fileobj=destination, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as archive:
            for name, data in selected.items():
                entry = tarfile.TarInfo("k-lani-link/" + name)
                entry.size, entry.mode, entry.mtime = len(data), 0o644, 0
                archive.addfile(entry, io.BytesIO(data))
    (output / "SOURCE-SHA256SUMS").write_bytes(manifest)
    print(f"Source bundle: {len(selected)} files; SHA256 {hashlib.sha256(archive_path.read_bytes()).hexdigest()}")
    print(f"Runtime state, keys, models and private recordings are excluded; {left_out} demo GIFs are left out. No publication performed.")


if __name__ == "__main__":
    main()
