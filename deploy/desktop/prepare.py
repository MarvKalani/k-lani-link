"""Copy Windows configuration into private Linux volumes; never expose pairing keys to the agent."""
import base64
import json
import os
from pathlib import Path, PurePosixPath
import secrets
import shutil
import stat
import tomllib
from urllib.parse import urlsplit


class Refused(ValueError):
    """A refusal whose fixed, secret-free text is shown to the operator."""


# Absolute paths of the HTTPS material inside the transport container.
HTTPS_STATE = "/var/lib/link/https"
HTTPS_TOKEN = "/run/secrets/relay-token"
HTTPS_CA = "/run/secrets/ca.pem"
CHAIN_FILES = {"client.chain": 8192, "client.chain.lock": 1024}


def normalize(config):
    # The Rust `check` command performs complete validation before any peer starts.
    allowed = {
        "signaling_url", "signaling_backend", "signaling_token", "room", "pair_key", "api_key",
        "pairing_state_dir", "listen", "upstream", "upstream_socket", "upstream_api_key",
        "relay", "ice_servers", "request_timeout_seconds", "allow_unverified_backend",
        "include_loopback_candidate", "max_concurrent_requests", "max_queued_requests",
        "management_token_file", "model_control_socket",
    }
    if set(config) - allowed:
        raise ValueError("unsupported configuration field")
    if config.get("relay", "disabled") != "disabled":
        raise ValueError("this profile requires TURN disabled")
    url = urlsplit(config.get("signaling_url", ""))
    if (url.scheme != "https" or not url.hostname or url.hostname in {"localhost", "127.0.0.1", "::1"}
            or url.username or url.password or url.query or url.fragment):
        raise ValueError("an external HTTPS signaling endpoint is required")
    servers = config.get("ice_servers", [])
    if not servers or any(not server.get("urls") for server in servers):
        raise ValueError("an explicit STUN server is required")
    if any(not uri.startswith(("stun:", "stuns:")) for server in servers for uri in server["urls"]):
        raise ValueError("only STUN is allowed in this profile")
    key = config.get("api_key")
    if not isinstance(key, str) or len(key) < 32:
        raise ValueError("invalid API credential")
    result = dict(config)
    for field in ("upstream_socket", "management_token_file", "model_control_socket"):
        result.pop(field, None)
    result.update(pairing_state_dir="/var/lib/link", listen="127.0.0.1:8787",
                  upstream="http://127.0.0.1:8080", upstream_api_key="", relay="disabled",
                  allow_unverified_backend=False, include_loopback_candidate=False)
    return result


def relative(value):
    """A path inside the client folder, as `init-https` writes it."""
    if not isinstance(value, str) or not value or "\\" in value or ":" in value:
        raise ValueError("invalid HTTPS file reference")
    if any(part in {"", ".", ".."} for part in value.split("/")):
        raise ValueError("invalid HTTPS file reference")
    return PurePosixPath(value)


def public_url(value, *, proxy=False):
    url = urlsplit(value) if isinstance(value, str) else None
    if (url is None or url.scheme not in ({"http", "https"} if proxy else {"https"}) or not url.hostname
            or url.query or url.fragment or (not proxy and (url.username or url.password))
            or url.hostname in {"localhost", "127.0.0.1", "::1"}):
        raise ValueError("invalid HTTPS relay or proxy URL")
    return value


def normalize_https(config, proxy=None):
    """The client half of `init-https`, rewritten to fixed container paths."""
    allowed = {
        "transport", "https", "signaling_url", "signaling_backend", "signaling_token", "room", "pair_key",
        "api_key", "listen", "upstream", "upstream_api_key", "relay", "ice_servers",
        "request_timeout_seconds", "max_concurrent_requests", "max_queued_requests", "resume_seconds",
    }
    if set(config) - allowed:
        raise ValueError("unsupported configuration field")
    if (any(config.get(field, "") != "" for field in ("signaling_url", "signaling_token", "room", "pair_key"))
            or config.get("signaling_backend", "mailbox") != "mailbox" or config.get("ice_servers", []) != []
            or config.get("relay", "disabled") != "disabled"):
        raise ValueError("HTTPS configuration must not carry WebRTC settings")
    key = config.get("api_key")
    if not isinstance(key, str) or len(key) < 32:
        raise ValueError("invalid API credential")
    https = config.get("https")
    if not isinstance(https, dict) or set(https) - {
            "relays", "state_dir", "admission_token_file", "proxy_url", "ca_cert_file", "poll_wait_seconds",
            "http1_only", "http3"}:
        raise ValueError("invalid HTTPS settings")
    relays = https.get("relays")
    if not isinstance(relays, list) or not relays:
        raise ValueError("an HTTPS relay is required")
    files = {"state": relative(https.get("state_dir")), "token": relative(https.get("admission_token_file"))}
    if "ca_cert_file" in https:
        files["ca"] = relative(https["ca_cert_file"])
    selected = {"relays": [public_url(relay) for relay in relays], "state_dir": HTTPS_STATE,
                "admission_token_file": HTTPS_TOKEN}
    proxy = proxy or https.get("proxy_url")
    if proxy:
        selected["proxy_url"] = public_url(proxy, proxy=True)
    if "ca" in files:
        selected["ca_cert_file"] = HTTPS_CA
    if "poll_wait_seconds" in https:
        selected["poll_wait_seconds"] = https["poll_wait_seconds"]
    for switch in ("http1_only", "http3"):
        if switch in https:
            if not isinstance(https[switch], bool):
                raise ValueError("invalid HTTPS settings")
            selected[switch] = https[switch]
    # QUIC cannot pass an HTTP proxy; never let it silently bypass the customer's proxy.
    if selected.get("http3") and "proxy_url" in selected:
        raise ValueError("HTTP/3 cannot be combined with a proxy")
    result = {"transport": "https", "signaling_backend": "mailbox", "api_key": key,
              "listen": "127.0.0.1:8787", "upstream": "http://127.0.0.1:8080", "upstream_api_key": "",
              "relay": "disabled"}
    for field in ("request_timeout_seconds", "max_concurrent_requests", "max_queued_requests", "resume_seconds"):
        if field in config:
            result[field] = config[field]
    result["https"] = selected
    return result, files


def toml(config):
    # The supported Rust configuration contains scalars, ICE-server arrays and the HTTPS table.
    def scalar(value):
        if isinstance(value, list) and all(isinstance(item, str) for item in value):
            return "[" + ", ".join(scalar(item) for item in value) + "]"
        if not isinstance(value, (str, bool, int)):
            raise ValueError("invalid configuration value")
        return json.dumps(value, ensure_ascii=True)
    lines = [f"{key} = {scalar(value)}" for key, value in config.items() if key not in {"ice_servers", "https"}]
    if "https" in config:
        lines += ["", "[https]"] + [f"{key} = {scalar(value)}" for key, value in config["https"].items()]
    for server in config.get("ice_servers", []):
        if set(server) - {"urls", "username", "credential"}:
            raise ValueError("unsupported ICE field")
        lines += ["", "[[ice_servers]]", "urls = [" + ", ".join(scalar(u) for u in server["urls"]) + "]"]
        lines += [f"{key} = {scalar(server[key])}" for key in ("username", "credential") if key in server]
    return "\n".join(lines) + "\n"


def private_write(path, value):
    temporary = path.with_suffix(".new")
    with temporary.open("x", encoding="utf-8") as output:
        os.chmod(temporary, 0o600)
        os.chown(temporary, 1000, 1000)
        output.write(value)
    temporary.replace(path)


def read_private(path, limit):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_size > limit:
        raise ValueError("invalid HTTPS file")
    return path.read_bytes()


def chain_files(folder):
    """The live pairing state of the client folder: exactly the files the Rust store keeps."""
    try:
        info = folder.lstat()
    except FileNotFoundError:
        raise Refused("The HTTPS pairing state is missing from the client folder; it was already "
                      "imported or never issued. Issue a new pair.") from None
    if not stat.S_ISDIR(info.st_mode):
        raise ValueError("invalid HTTPS state folder")
    names = {entry.name for entry in folder.iterdir()}
    if "client.chain" not in names or names - set(CHAIN_FILES):
        raise ValueError("unexpected HTTPS state files")
    return {name: read_private(folder / name, CHAIN_FILES[name]) for name in sorted(names)}


def prepare_https(folder, config, output, proxy):
    """Import a portable HTTPS client folder once. The volume copy then is the only live state."""
    config, files = normalize_https(config, proxy)
    state = output / "state/https"
    previous = output / "link/client.toml"
    # Decide everything before the first write.
    if state.exists() or state.is_symlink():
        if not previous.is_file():
            raise Refused("HTTPS pairing state exists without its configuration; refusing to guess.")
        if tomllib.loads(previous.read_text()).get("api_key") != config["api_key"]:
            raise Refused("A different HTTPS pair is already imported. Remove the pairing volumes "
                          "deliberately before importing another pair.")
        imported = None
    else:
        imported = chain_files(folder / files["state"])
    token = read_private(folder / files["token"], 4096).decode("utf-8").strip()
    if not token:
        raise ValueError("invalid relay token")
    certificate = read_private(folder / files["ca"], 128 * 1024).decode("utf-8") if "ca" in files else None
    return config, token, certificate, imported


def import_state(output, chain):
    temporary = output / "state" / (".https-import-" + secrets.token_hex(8))
    temporary.mkdir(mode=0o700)
    try:
        os.chown(temporary, 1000, 1000)
        for name, data in chain.items():
            path = temporary / name
            with path.open("xb") as handle:
                os.chmod(path, 0o600)
                os.chown(path, 1000, 1000)
                handle.write(data)
        temporary.rename(output / "state/https")
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise


def prepare(source, output, proxy=None):
    info = source.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_size > 65536:
        raise ValueError("invalid input file")
    loaded = tomllib.loads(source.read_text(encoding="utf-8-sig"))
    https = loaded.get("transport") == "https"
    if https:
        config, token, certificate, chain = prepare_https(source.parent, loaded, output, proxy)
    else:
        config = normalize(loaded)
    content = toml(config)
    for name in ("link", "app", "ui", "state", "api", "socket"):
        directory = output / name
        if directory.is_symlink():
            raise ValueError("invalid volume")
        directory.mkdir(exist_ok=True)
        os.chown(directory, 1000, 1000)
        os.chmod(directory, 0o700)
    password = secrets.token_urlsafe(32)
    private_write(output / "link/client.toml", content)
    private_write(output / "app/client.toml", toml({"api_key": config["api_key"]}))
    private_write(output / "app/server-password.txt", password + "\n")
    basic = base64.b64encode(("opencode:" + password).encode()).decode()
    private_write(output / "ui/proxy-auth.conf", f'proxy_set_header Authorization "Basic {basic}";\n')
    if not https:
        return None
    private_write(output / "link/relay-token", token + "\n")
    ca = output / "link/ca.pem"
    if certificate is None:
        ca.unlink(missing_ok=True)
    else:
        private_write(ca, certificate)
    # Configuration first: an interrupted import leaves no state, so the next start imports again.
    if chain is None:
        return "KEPT"
    import_state(output, chain)
    return "IMPORTED"


# Where the client folder is mounted: a Windows folder, or the volume `join` wrote into.
INPUTS = {"/input", "/joined/client"}


def source_path():
    # The client's own folder is mounted read-only; HTTPS needs its token, CA and state too.
    folder = os.environ.get("LINK_INPUT_DIR", "/input")
    name = os.environ.get("LINK_CLIENT_FILE", "client.toml")
    if folder not in INPUTS or not name or "/" in name or "\\" in name or name in {".", ".."}:
        raise ValueError("invalid client file name")
    return Path(folder) / name



if __name__ == "__main__":
    try:
        os.umask(0o077)
        result = prepare(source_path(), Path("/output"), os.environ.get("LINK_PROXY_URL") or None)
    except Refused as refusal:
        raise SystemExit(str(refusal)) from None
    except (OSError, ValueError, TypeError, KeyError, UnicodeDecodeError):
        raise SystemExit("Desktop preparation failed; check the private config and its transport "
                         "settings (STUN/signaling or HTTPS relay, token and state).") from None
    print("Private desktop configuration prepared; no credentials displayed.")
    if result:
        # One fixed marker line for the launcher.
        print("HTTPS pairing state: " + result)
