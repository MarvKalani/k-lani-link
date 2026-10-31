"""Allowlisted machine/runtime metadata. Never collect host names, serials, UUIDs, addresses or secrets."""
import csv
import json
import os
from pathlib import Path
import platform
import subprocess


def command(args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.DEVNULL, timeout=5).strip()
    except (OSError, subprocess.SubprocessError):
        return ""


def os_release(text):
    values = {}
    for line in text.splitlines():
        key, sep, value = line.partition("=")
        if sep and key in {"PRETTY_NAME", "ID", "VERSION_ID"}:
            values[key] = value.strip().strip('"')
    return values


def read(path):
    try:
        return Path(path).read_text().strip()
    except OSError:
        return None


def collect(docker, identities, extra_path=None):
    try:
        cpu = {row["field"].rstrip(":"): row["data"] for row in json.loads(command(["lscpu", "--json"]))["lscpu"]}
    except (ValueError, KeyError):
        cpu = {}
    memory = read("/proc/meminfo") or ""
    visible_ram = next((int(line.split()[1]) * 1024 for line in memory.splitlines() if line.startswith("MemTotal:")), None)
    machine = {"cpu": cpu.get("Model name", platform.processor()), "logical_cpus": os.cpu_count(),
        "sockets": cpu.get("Socket(s)"), "cores_per_socket": cpu.get("Core(s) per socket"),
        "threads_per_core": cpu.get("Thread(s) per core"), "architecture": platform.machine(),
        "os": os_release(read("/etc/os-release") or ""), "kernel": platform.release(),
        "os_visible_memory_bytes": visible_ram,
        "board_vendor": read("/sys/class/dmi/id/board_vendor"), "board_model": read("/sys/class/dmi/id/board_name")}
    machine["disks"] = [{"device": p.name, "model": read(p / "device/model"),
        "capacity_bytes": int(read(p / "size") or 0) * 512} for p in sorted(Path("/sys/block").glob("nvme*n*"))]
    gpu = command(["nvidia-smi", "--query-gpu=name,driver_version,memory.total,power.limit", "--format=csv,noheader,nounits"])
    machine["gpus"] = []
    for row in csv.reader(gpu.splitlines()):
        if len(row) == 4:
            try:
                name, driver, memory_mib, limit = [value.strip() for value in row]
                machine["gpus"].append({"name": name, "driver": driver, "memory_mib": float(memory_mib), "power_limit_watts": float(limit)})
            except ValueError:
                pass
    # Optional operator-supplied DMI snapshot; import only its public hardware fields
    # and only on the CPU it was collected from. No automatic sudo is performed.
    if extra_path and extra_path.is_file():
        extra = json.loads(extra_path.read_text())
        if extra.get("cpu") == machine["cpu"]:
            machine["installed_memory_bytes"] = extra.get("installed_memory_bytes")
            keys = {"size_bytes", "type", "configured_speed_mts", "manufacturer", "part_number"}
            machine["memory_modules"] = [{k: v for k, v in module.items() if k in keys}
                                          for module in extra.get("memory_modules", [])]
    version = docker.get("/version")
    runtime = {"docker_engine": version.get("Version"), "docker_api": version.get("ApiVersion"),
        "docker_compose": command(["docker", "compose", "version", "--short"]), "containers": {}}
    for name, identity in identities.items():
        env = identity.get("Config", {}).get("Env") or []
        runtime["containers"][name] = {"image": identity["Config"]["Image"], "image_digest": identity["Image"],
            "os": os_release(command(["docker", "exec", identity["Id"], "cat", "/etc/os-release"])),
            "cuda_runtime": next((v.split("=", 1)[1] for v in env if v.startswith("CUDA_VERSION=")), None),
            "network_mode": identity["HostConfig"]["NetworkMode"], "memory_limit_bytes": identity["HostConfig"]["Memory"],
            "swap_allowance_bytes": max(0, identity["HostConfig"]["MemorySwap"] - identity["HostConfig"]["Memory"])}
    return {"machine": machine, "runtime": runtime}
