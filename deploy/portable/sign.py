"""Sign a release's SHA256SUMS for the built-in updater, or create the signing key once.

  python3 -B deploy/portable/sign.py --generate /private/update-signing.pem
  python3 -B deploy/portable/sign.py --key /private/update-signing.pem out/SHA256SUMS

Signing writes SHA256SUMS.sig (base64 Ed25519 signature) next to the file. The private
key never belongs in the repository; the public key printed by --generate goes into
src/update.rs. Losing the key stops updates for every installed copy; a leaked key lets
anyone ship code to them. Keep an offline backup.
"""
import argparse
import base64
import os
from pathlib import Path

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def public(key):
    raw = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    return base64.b64encode(raw).decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--generate", type=Path, help="write a new private key here (never overwrites)")
    parser.add_argument("--key", type=Path, help="the private key that signs")
    parser.add_argument("sums", type=Path, nargs="?", help="the SHA256SUMS file to sign")
    args = parser.parse_args()
    if args.generate:
        key = Ed25519PrivateKey.generate()
        pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                serialization.NoEncryption())
        descriptor = os.open(args.generate, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(pem)
        print("Public key for src/update.rs:", public(key))
        return
    if not (args.key and args.sums):
        parser.error("name --key and the SHA256SUMS file, or use --generate")
    key = serialization.load_pem_private_key(args.key.read_bytes(), password=None)
    if not isinstance(key, Ed25519PrivateKey):
        parser.error("not an Ed25519 key")
    data = args.sums.read_bytes()
    signature = base64.b64encode(key.sign(data)).decode()
    args.sums.with_name(args.sums.name + ".sig").write_text(signature + "\n", encoding="ascii")
    print(f"Signed {args.sums.name} with key {public(key)}")


if __name__ == "__main__":
    main()
