"""Container-only startup: no network interfaces, writable output or diagnostic switches."""
import os
from pathlib import Path
import resource
import socket
import stat
import sys


def main():
    sink = os.open(os.devnull, os.O_WRONLY)
    os.dup2(sink, 1)
    os.dup2(sink, 2)
    if sink not in (1, 2):
        os.close(sink)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    if os.environ.get("LINK_REQUIRE_NO_SWAP") == "1" and len(Path("/proc/swaps").read_text().splitlines()) != 1:
        return 2
    # Refuse a deployment that accidentally restores an Ethernet/network interface.
    if {name for _, name in socket.if_nameindex()} - {"lo"}:
        return 2
    path = "/run/link/basalt.sock"
    try:
        info = os.lstat(path)
        if not stat.S_ISSOCK(info.st_mode) or info.st_uid != os.getuid():
            return 2
        os.unlink(path)
    except FileNotFoundError:
        pass
    # Verify the privacy mode exists before running a potentially updated Basalt tree.
    sys.path.insert(0, "/opt/basalt")
    from serve.config import build_parser
    if "--privacy" not in build_parser()._option_string_actions:
        return 2
    metrics = []
    if os.environ.get("LINK_NUMERIC_METRICS") == "1":
        if "--privacy-metrics-file" not in build_parser()._option_string_actions:
            return 2
        metrics = ["--privacy-metrics-file", "/run/link/performance.json"]
    args = ["/opt/basalt/bin/basalt", "serve", "--config", "/run/secrets/basalt.json",
            *sys.argv[1:], "--privacy", "--vision", "off", "--host", "127.0.0.1", "--unix-socket", path]
    args.extend(metrics)
    os.execv(args[0], args)


try:
    raise SystemExit(main())
except Exception:
    raise SystemExit(1) from None
