"""Prepare synthetic inputs only; the recorded Basalt agent creates all deliverables."""
import argparse
import os
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT / ".opencode-test")
    args = parser.parse_args()
    os.umask(0o077)
    project = args.root.resolve(strict=True) / "work/demo"
    project.mkdir(mode=0o700)
    (project / "assets").mkdir(mode=0o700)
    (project / "output").mkdir(mode=0o700)
    for name in ("BRIEFING.md", "customer.json"):
        shutil.copy2(ROOT / "examples/business-demo" / name, project / name)
    shutil.copy2(ROOT / "assets/k-lani-link.webp", project / "assets/k-lani-link.webp")
    (args.root / "craft.profile").touch(mode=0o600)
    print("Synthetic inputs ready at /work/demo; output/ is empty. Craft profile selected.")


if __name__ == "__main__":
    main()
