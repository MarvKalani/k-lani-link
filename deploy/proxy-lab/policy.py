"""Small, fail-closed LAB policy; no claim to reproduce a vendor's URL reputation feed."""
import base64
import hashlib
import json
from urllib.parse import urlsplit

QUESTION = b"QUESTION_KLANI_PROXY_LAB_"
ANSWER = b"ANSWER_KLANI_PROXY_LAB_"


def permitted(policy, host, port):
    if not isinstance(policy, dict):
        return False
    hosts = policy.get("hosts")
    return (policy.get("allow") is True and port == 443 and isinstance(hosts, list)
            and all(isinstance(name, str) for name in hosts) and host.lower() in hosts)


def request_status(policy, host, port, method, path, content_type, scheme="https"):
    if scheme != "https" or not permitted(policy, host, port):
        return 403
    if not urlsplit(path).path.startswith("/v1/"):
        return 403
    if method not in {"GET", "PUT", "POST"}:
        return 405
    if policy.get("mode") in {"post_only", "json_only"} and method == "PUT":
        return 405
    if (policy.get("mode") == "json_only" and method == "POST"
            and content_type.split(";", 1)[0] != "application/json"):
        return 415
    return None


def payload_summary(body, content_type=""):
    """Check the actual record too: base64 wrapping alone must not hide a plaintext leak."""
    record = body
    if content_type.split(";", 1)[0] == "application/json":
        try:
            value = json.loads(body)
            if isinstance(value, dict) and isinstance(value.get("data"), str):
                value = value["data"]
                record = base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
        except (ValueError, TypeError, UnicodeDecodeError):
            pass
    return {
        "bytes": len(body),
        "record_sha256": hashlib.sha256(record).hexdigest(),
        "plaintext_marker": any(marker in data for marker in (QUESTION, ANSWER)
                                for data in (body, record)),
    }
