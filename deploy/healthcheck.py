"""Quiet socket health probe: Docker stores only exit status, never response content."""
from pathlib import Path
import socket
import sys

try:
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(2)
        managed = Path("/run/link/control.sock").exists()
        connection.connect("/run/link/control.sock" if managed else "/run/link/basalt.sock")
        path = "/v1/link/models" if managed else "/health"
        connection.sendall(f"GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n".encode())
        status = connection.recv(128).split(b"\r\n", 1)[0]
        ok = b" 200 " in status
except Exception:
    ok = False
sys.exit(0 if ok else 1)
