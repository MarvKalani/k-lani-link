"""Verify the running lab's isolation and streaming configuration without storing chat content."""
import http.client
import json
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent


def run(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, text=True, timeout=30, **kwargs).stdout


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=HERE.parents[1] / ".thinkrail-test")
    args = parser.parse_args()
    compose = ["docker", "compose", "--env-file", str(args.root.resolve() / "compose.env"), "-f", str(HERE / "compose.yaml")]
    ids = {name: run([*compose, "ps", "--all", "-q", name]).strip()
        for name in ("basalt", "thinkrail", "client", "host", "ui", "guard", "signal")}
    for name, identity in ids.items():
        inspected = json.loads(run(["docker", "inspect", identity]))[0]
        config = inspected["HostConfig"]
        assert config["ReadonlyRootfs"] and config["LogConfig"]["Type"] == "none", name
        assert not config["Privileged"] and "ALL" in config["CapDrop"], name
        assert not any(m["Destination"] == "/var/run/docker.sock" for m in inspected["Mounts"]), name
        assert inspected["State"]["Running"] == (name != "signal"), name
        if name in ("basalt", "thinkrail"):
            assert config["NetworkMode"] == "none" and not config["PortBindings"], name
        if name != "guard":
            assert not config.get("CapAdd"), name
    network_probe = """import socket,struct,secrets
for address,port in [('1.1.1.1',443),('8.8.8.8',53),('2606:4700:4700::1111',443)]:
 try:
  connection=socket.create_connection((address,port),timeout=0.5)
 except OSError:pass
 else:
  connection.close();raise AssertionError('outgoing TCP was allowed')
query=struct.pack('!HHHHHH',secrets.randbits(16),0x0100,1,0,0,0)+b'\\x07example\\x03com\\x00'+struct.pack('!HH',1,1)
for address in ['127.0.0.11','8.8.8.8']:
 with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as connection:
  connection.settimeout(0.6)
  try:
   connection.sendto(query,(address,53));connection.recv(2048)
  except OSError:pass
  else:raise AssertionError('DNS escaped the isolation boundary')
"""
    for name in ("basalt", "thinkrail", "client"):
        python = "/opt/basalt/.venv/bin/python" if name == "basalt" else "python"
        run(["docker", "exec", "-i", ids[name], python, "-B", "-"], input=network_probe)
    model_check = """import json
from pathlib import Path
model=json.loads((Path.home()/'.pi/agent/models.json').read_text())['providers']['link']['models'][0]
assert model['contextWindow']==262144 and model['reasoning']
print(json.dumps({'model':model['id'],'context_tokens':model['contextWindow']}))
"""
    model = json.loads(run(["docker", "exec", "-i", ids["thinkrail"], "python", "-B", "-"], input=model_check))
    for path, headers, expected in [("/", {}, 200), ("/stats", {}, 200),
            ("/ws", {"Origin": "https://untrusted.example.invalid"}, 403),
            ("/", {"Host": "untrusted.example.invalid"}, 403)]:
        connection = http.client.HTTPConnection("127.0.0.1", 24242, timeout=3)
        try:
            connection.request("GET", path, headers=headers)
            response = connection.getresponse()
            assert response.status == expected
            if expected == 200:
                assert "connect-src 'self'" in response.getheader("Content-Security-Policy", "")
        finally:
            connection.close()
    print(json.dumps({"verified": True, **model, "signaling_stopped": True,
        "outbound_ipv4_ipv6_dns_blocked": True, "local_ui_origin_checks": True}))


if __name__ == "__main__":
    main()
