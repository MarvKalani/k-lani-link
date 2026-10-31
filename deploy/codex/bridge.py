"""Loopback HTTP access to one k-lani-link Unix socket, inside a network=none container."""
import os
import socket
import subprocess
import sys
import threading


def forward(source, destination):
    try:
        while chunk := source.recv(16384):
            destination.sendall(chunk)
    except OSError:
        pass
    finally:
        try:
            destination.shutdown(socket.SHUT_WR)
        except OSError:
            pass


def connection(client, path):
    with client, socket.socket(socket.AF_UNIX) as upstream:
        try:
            upstream.connect(path)
        except OSError:
            return
        upload = threading.Thread(target=forward, args=(client, upstream), daemon=True)
        upload.start()
        forward(upstream, client)
        upload.join(timeout=1)


def accept(listener, path):
    while True:
        try:
            client, _ = listener.accept()
        except OSError:
            return
        threading.Thread(target=connection, args=(client, path), daemon=True).start()


def main():
    if {name for _, name in socket.if_nameindex()} != {"lo"}:
        return 2
    path = os.environ.get("LINK_API_SOCKET", "/run/link/api.sock")
    port = int(os.environ.get("LINK_API_PORT", "8787"))
    if not os.path.isabs(path) or not sys.argv[1:]:
        return 2
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", port))
        listener.listen(8)
        threading.Thread(target=accept, args=(listener, path), daemon=True).start()
        return subprocess.call(sys.argv[1:], stdin=sys.stdin, stdout=sys.stdout, stderr=sys.stderr)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError):
        raise SystemExit(2) from None
