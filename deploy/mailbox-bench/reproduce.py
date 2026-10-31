"""Run desired-behavior regressions for the three original state counterexamples.

No temporary test injection, network or real credentials. The historical reproductions
at revision 0d92e20 asserted the broken behavior; these tests assert that it stays fixed.
"""
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]

if __name__ == "__main__":
    subprocess.run(["cargo", "test", "--test", "mailbox_state", "--locked"], cwd=ROOT, check=True)
