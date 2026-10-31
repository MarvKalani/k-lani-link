"""Verify the live second-client boundary, swap policy, backend storage and configured model."""
import argparse
import http.client
import json
from pathlib import Path
import subprocess


def run(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, text=True, timeout=30, **kwargs).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    assert len(Path("/proc/swaps").read_text().splitlines()) == 1, "host swap is enabled"
    inspections = {}
    for name in ("opencode", "ui", "guard"):
        inspections[name] = json.loads(run(["docker", "inspect", f"k-lani-opencode-test-{name}-1"]))[0]
    inspections["basalt"] = json.loads(run(["docker", "inspect", "k-lani-thinkrail-test-basalt-1"]))[0]
    for name, info in inspections.items():
        host = info["HostConfig"]
        assert info["State"]["Running"] and not info["State"]["OOMKilled"], name
        assert host["ReadonlyRootfs"] and host["LogConfig"]["Type"] == "none" and not host["Privileged"], name
        assert "ALL" in host["CapDrop"] and (name == "guard" or not host.get("CapAdd")), name
        assert not any(m["Destination"] == "/var/run/docker.sock" for m in info["Mounts"]), name
        if name in ("opencode", "basalt"):
            assert host["NetworkMode"] == "none" and not host.get("PortBindings"), name
            assert host["Memory"] > 0 and host["MemorySwap"] == host["Memory"], name
    signal = json.loads(run(["docker", "inspect", "k-lani-thinkrail-test-signal-1"]))[0]
    assert not signal["State"]["Running"]
    probe = """import socket,struct,secrets
assert {n for _,n in socket.if_nameindex()} == {'lo'}
for address,port in [('1.1.1.1',443),('8.8.8.8',53),('2606:4700:4700::1111',443)]:
 try:connection=socket.create_connection((address,port),timeout=.5)
 except OSError:pass
 else:connection.close();raise AssertionError('TCP egress allowed')
query=struct.pack('!HHHHHH',secrets.randbits(16),0x0100,1,0,0,0)+b'\\x07example\\x03com\\x00'+struct.pack('!HH',1,1)
for address in ['127.0.0.11','8.8.8.8']:
 with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as connection:
  connection.settimeout(.6)
  try:connection.sendto(query,(address,53));connection.recv(2048)
  except OSError:pass
  else:raise AssertionError('DNS egress allowed')
"""
    for name, python in (("opencode", "python"), ("basalt", "/opt/basalt/.venv/bin/python")):
        run(["docker", "exec", "-i", inspections[name]["Id"], python, "-B", "-"], input=probe)
    # Probe the UI namespace too: Docker's DNS DNAT must not bypass its loopback rule.
    ui_probe = probe.replace("assert {n for _,n in socket.if_nameindex()} == {'lo'}\n", "")
    run(["docker", "run", "--rm", "--network", "container:" + inspections["guard"]["Id"], "--read-only",
        "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--log-driver", "none", "-i",
        "--entrypoint", "python", "k-lani-link-host:local", "-B", "-"], input=ui_probe)
    for port, path, headers, expected in [(24244, "/", {}, 200), (24245, "/stats", {}, 200),
            (24244, "/", {"Origin": "https://untrusted.example.invalid"}, 403),
            (24244, "/", {"Host": "untrusted.example.invalid"}, 403),
            (24245, "/", {"Origin": "https://untrusted.example.invalid"}, 403),
            (24245, "/", {"Host": "untrusted.example.invalid"}, 403)]:
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
        try:
            connection.request("GET", path, headers=headers)
            response = connection.getresponse()
            assert response.status == expected
            if expected == 200:
                assert "connect-src 'self'" in response.getheader("Content-Security-Policy", "")
                if port == 24245:
                    assert b'LOCAL MEASUREMENT' in response.read()
        finally:
            connection.close()
    model_mount = next(m for m in inspections["basalt"]["Mounts"] if m["Destination"] == "/models/model.basalt")
    assert not model_mount["RW"]
    source = Path(model_mount["Source"]).resolve(strict=True)
    storage = run(["findmnt", "--noheadings", "--output", "SOURCE,FSTYPE", "--target", str(source)]).strip()
    model_probe = """import json,os
from pathlib import Path
mapped=False
for p in Path('/proc').iterdir():
 if not p.name.isdigit():continue
 try:mapped = mapped or '/models/model.basalt' in (p/'maps').read_text()
 except OSError:pass
events=dict(line.split() for line in Path('/sys/fs/cgroup/memory.events').read_text().splitlines())
assert int(events['oom_kill'])==0
print(json.dumps({'file_device_id':os.stat('/models/model.basalt').st_dev,'mapped_model_file':mapped}))
"""
    file_evidence = json.loads(run(["docker", "exec", "-i", inspections["basalt"]["Id"], "/opt/basalt/.venv/bin/python", "-B", "-"], input=model_probe))
    assert file_evidence["file_device_id"] == source.stat().st_dev and file_evidence["mapped_model_file"]
    cfg_probe = """from pathlib import Path
import json
c=json.loads((Path.home()/'.config/opencode/opencode.json').read_text())
p=c['providers']['link'];m=p['models']['basalt']
assert p['settings']['baseURL']=='http://127.0.0.1:8787/v1' and m['limit']['context']==262144
assert c['share']=='disabled' and c['update']=='disable'
print(json.dumps({'model_id':m['modelID'],'context_tokens':m['limit']['context']}))
"""
    model = json.loads(run(["docker", "exec", "-i", inspections["opencode"]["Id"], "python", "-B", "-"], input=cfg_probe))
    report = {"verified": True, "host_swap_disabled": True, "container_swap_disabled": True,
        "outbound_ipv4_ipv6_dns_blocked": True, "signaling_stopped": True, "host_origin_checks": True,
        "model_file": source.name, "storage": storage, **file_evidence, **model}
    if args.output:
        args.output.write_text(json.dumps(report, indent=2))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
