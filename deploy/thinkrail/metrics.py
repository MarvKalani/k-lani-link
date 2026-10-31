"""Read-only resource sampler for the two application containers. Never reads inference traffic."""
import argparse
import csv
from datetime import datetime, timezone
import http.client
import io
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tarfile
import time
from hardware import collect as collect_hardware
from performance import sanitize


class Docker:
    def __init__(self, path="/var/run/docker.sock"):
        self.path = path

    def get(self, route):
        connection = http.client.HTTPConnection("localhost", timeout=4)
        connection.sock = socket.socket(socket.AF_UNIX)
        connection.sock.settimeout(4)
        connection.sock.connect(self.path)
        try:
            connection.request("GET", route)
            response = connection.getresponse()
            if response.status != 200:
                raise RuntimeError("Docker read returned HTTP " + str(response.status))
            return json.loads(response.read())
        finally:
            connection.close()

    def performance(self, identity):
        """Read one bounded numeric tmpfs file; never execute code or read request logs."""
        connection = http.client.HTTPConnection("localhost", timeout=4)
        connection.sock = socket.socket(socket.AF_UNIX)
        connection.sock.settimeout(4)
        try:
            connection.sock.connect(self.path)
            connection.request("GET", "/containers/" + identity["Id"] + "/archive?path=/run/link/performance.json")
            response = connection.getresponse()
            payload = response.read(65537)
            if response.status != 200 or len(payload) > 65536:
                return sanitize(None, 0, 0)
            with tarfile.open(fileobj=io.BytesIO(payload)) as archive:
                entries = archive.getmembers()
                if len(entries) != 1 or not entries[0].isfile() or entries[0].name != "performance.json" or entries[0].size > 16384:
                    return sanitize(None, 0, 0)
                raw = json.load(archive.extractfile(entries[0]))
            started = datetime.fromisoformat(identity["State"]["StartedAt"].replace("Z", "+00:00")).timestamp()
            return sanitize(raw, time.time(), started)
        except (OSError, ValueError, KeyError, tarfile.TarError, http.client.HTTPException):
            return sanitize(None, 0, 0)
        finally:
            connection.close()


def normalize(raw, previous=None):
    cpu = raw.get("cpu_stats", {})
    usage = cpu.get("cpu_usage", {}).get("total_usage", 0)
    system = cpu.get("system_cpu_usage", 0)
    percent = None
    if previous:
        total_delta, system_delta = usage - previous[0], system - previous[1]
        if total_delta >= 0 and system_delta > 0:
            percent = total_delta / system_delta * cpu.get("online_cpus", 1) * 100
    memory = raw.get("memory_stats", {})
    memory_stats = memory.get("stats", {})
    inactive = memory_stats.get("inactive_file", memory_stats.get("total_inactive_file", 0))
    io = raw.get("blkio_stats", {}).get("io_service_bytes_recursive") or []
    networks = raw.get("networks", {}).values()
    result = {"cpu_percent": round(percent, 3) if percent is not None else None,
        "memory_usage_bytes": memory.get("usage"),
        "memory_working_set_bytes": max(0, memory.get("usage", 0) - inactive),
        "memory_inactive_file_bytes": inactive, "memory_limit_bytes": memory.get("limit"),
        "block_read_bytes": sum(e.get("value", 0) for e in io if e.get("op", "").lower() == "read"),
        "block_write_bytes": sum(e.get("value", 0) for e in io if e.get("op", "").lower() == "write"),
        "network_rx_bytes": sum(n.get("rx_bytes", 0) for n in networks),
        "network_tx_bytes": sum(n.get("tx_bytes", 0) for n in networks),
        "pids": raw.get("pids_stats", {}).get("current")}
    return result, (usage, system)


def gpu_sample(basalt_pids):
    try:
        raw = subprocess.check_output(["nvidia-smi", "--query-gpu=name,memory.used,memory.total,utilization.gpu,power.draw,power.limit",
            "--format=csv,noheader,nounits"], text=True, stderr=subprocess.DEVNULL, timeout=4)
        processes = subprocess.check_output(["nvidia-smi", "--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"],
            text=True, stderr=subprocess.DEVNULL, timeout=4)
        rows = list(csv.reader(raw.splitlines()))
        # The demo uses one GPU; avoid presenting an arbitrary multi-GPU aggregate.
        if len(rows) != 1:
            return None
        name, used, total, utilization, power, limit = [v.strip() for v in rows[0]]
        own = [float(memory.strip()) for pid, memory in csv.reader(processes.splitlines()) if pid.strip() in basalt_pids]
        return {"name": name, "memory_used_mib": float(used), "memory_total_mib": float(total),
            "utilization_percent": float(utilization), "power_watts": float(power), "power_limit_watts": float(limit),
            "basalt_process_memory_mib": sum(own) if own else None}
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def host_sample(previous):
    try:
        values = [int(v) for v in Path("/proc/stat").read_text().splitlines()[0].split()[1:9]]
        total, idle = sum(values), values[3] + values[4]
        percent = None
        if previous and total > previous[0]:
            percent = (1 - (idle - previous[1]) / (total - previous[0])) * 100
        memory = {}
        for line in Path("/proc/meminfo").read_text().splitlines():
            key, rest = line.split(":", 1)
            memory[key] = int(rest.split()[0]) * 1024
        vm = {line.split()[0]: int(line.split()[1]) for line in Path("/proc/vmstat").read_text().splitlines()}
        return {"cpu_percent": round(percent, 3) if percent is not None else None,
            "memory_total_bytes": memory["MemTotal"], "memory_available_bytes": memory["MemAvailable"],
            "swap_used_bytes": memory["SwapTotal"] - memory["SwapFree"], "swap_total_bytes": memory["SwapTotal"],
            "swap_in_pages": vm.get("pswpin"), "swap_out_pages": vm.get("pswpout")}, (total, idle)
    except (OSError, KeyError, ValueError):
        return None, previous


def save(path, value):
    temporary = path.with_suffix(path.suffix + ".new")
    temporary.write_text(json.dumps(value, separators=(",", ":")))
    temporary.replace(path)


def model_metadata(identity, performance=None):
    labels = identity.get("Config", {}).get("Labels", {})
    managed = any(m.get("Destination") == "/run/secrets/models.json" for m in identity.get("Mounts", []))
    active = (performance or {}).get("model") if (performance or {}).get("available") and (performance or {}).get("state") != "unloaded" else None
    destination = f"/models/{active}.basalt" if managed and active else None if managed else "/models/model.basalt"
    mounts = [m for m in identity.get("Mounts", []) if destination is not None and m.get("Destination") == destination]
    result = {"preset": labels.get("com.k-lani-link.model-preset", "unspecified"),
        "context_tokens": labels.get("com.k-lani-link.context-tokens"),
        "memory_profile": labels.get("com.k-lani-link.memory-profile", "default")}
    if managed:
        result["preset"] = active or "unloaded / loading"
        result["context_tokens"] = (performance or {}).get("context_tokens_per_slot")
    if len(mounts) == 1:
        path = Path(mounts[0]["Source"])
        result["file_name"] = path.name
        result["file_size_bytes"] = path.stat().st_size
        result["filesystem_device_id"] = path.stat().st_dev
        try:
            result["storage_device"] = subprocess.check_output(["findmnt", "--noheadings", "--output", "SOURCE", "--target", str(path)],
                text=True, stderr=subprocess.DEVNULL, timeout=3).strip()
        except (OSError, subprocess.SubprocessError):
            result["storage_device"] = None
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", default="k-lani-thinkrail-test")
    parser.add_argument("--agent", choices=("thinkrail", "opencode"), default="thinkrail")
    parser.add_argument("--backend-project", help="Compose project containing the shared Basalt container")
    parser.add_argument("--require-no-swap", action="store_true")
    parser.add_argument("--live", type=Path, required=True)
    parser.add_argument("--output", type=Path, help="New recording directory; omitted: live values only")
    parser.add_argument("--hardware-extra", type=Path, help="Optional allowlisted RAM-module metadata collected by the operator")
    parser.add_argument("--interval", type=float, default=1)
    parser.add_argument("--duration", type=float, default=14400, help="Maximum seconds, 0 waits for Ctrl+C")
    args = parser.parse_args()
    if not 0.5 <= args.interval <= 60 or args.duration < 0:
        parser.error("invalid sampling interval or duration")
    os.umask(0o077)
    args.live.mkdir(parents=True, exist_ok=True, mode=0o700)
    docker = Docker()
    projects = {args.agent: args.project, "basalt": args.backend_project or args.project}
    names = {name: project + "-" + name + "-1" for name, project in projects.items()}
    identities = {name: docker.get("/containers/" + value + "/json") for name, value in names.items()}
    for name, value in identities.items():
        if not value.get("State", {}).get("Running"):
            raise RuntimeError("both application containers must be running before recording")
        if value.get("Config", {}).get("Labels", {}).get("com.docker.compose.project") != projects[name]:
            raise RuntimeError("container does not belong to the selected demo")
    metadata = {"schema": 2, "started_at": datetime.now(timezone.utc).isoformat(), "interval_seconds": args.interval,
        "agent": args.agent, "requires_no_swap": args.require_no_swap,
        "model": model_metadata(identities["basalt"]),
        "containers": {k: {"image_digest": v["Image"], "id": v["Id"][:12]} for k, v in identities.items()},
        "cpu_percent_definition": "100 percent is one CPU core", "memory_working_set_definition": "usage minus inactive_file",
        "network_counters_exclude_unix_sockets": True,
        "gpu_utilization_and_power_scope": "whole GPU; only process VRAM is attributed to Basalt",
        "host_metrics_include_other_programs": True, "contains_prompts_or_credentials": False}
    metadata["hardware"] = collect_hardware(docker, identities, args.hardware_extra)
    files = []
    if args.output:
        args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
        save(args.output / "metadata.json", metadata)
        samples = (args.output / "samples.jsonl").open("w", buffering=1)
        csv_file = (args.output / "containers.csv").open("w", newline="", buffering=1)
        system_file = (args.output / "system.csv").open("w", newline="", buffering=1)
        performance_file = (args.output / "performance.csv").open("w", newline="", buffering=1)
        files = [samples, csv_file, system_file, performance_file]
        performance_writer = csv.DictWriter(performance_file, fieldnames=["timestamp_utc", "elapsed_seconds", "available",
            "state", "model", "live_decode_tokens_per_second", "live_prefill_tokens_per_second", "generated_tokens",
            "active_requests", "last_decode_tokens_per_second", "last_prefill_tokens_per_second", "last_output_tokens", "last_ttft_ms"])
        performance_writer.writeheader()
        columns = ["timestamp_utc", "elapsed_seconds", "container", "cpu_percent", "memory_usage_bytes",
            "memory_working_set_bytes", "memory_inactive_file_bytes", "memory_limit_bytes", "block_read_bytes",
            "block_write_bytes", "network_rx_bytes", "network_tx_bytes", "pids"]
        writer = csv.DictWriter(csv_file, fieldnames=columns)
        writer.writeheader()
        system_writer = csv.DictWriter(system_file, fieldnames=["timestamp_utc", "elapsed_seconds", "host_cpu_percent",
            "host_memory_available_bytes", "host_swap_used_bytes", "host_swap_total_bytes", "host_swap_in_pages", "host_swap_out_pages", "gpu_utilization_percent", "gpu_power_watts",
            "gpu_memory_used_mib", "basalt_gpu_memory_mib"])
        system_writer.writeheader()
    stopped = False
    def stop(*_):
        nonlocal stopped
        stopped = True
    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    previous, previous_host = {}, None
    start = time.monotonic()
    summary = {"samples": 0, "containers": {name: {"cpu_max_percent": 0, "memory_max_bytes": 0} for name in names}}
    (args.live / "recorder.pid").write_text(str(os.getpid()))
    try:
        while not stopped and (args.duration == 0 or time.monotonic() - start < args.duration):
            tick = time.monotonic()
            current = {"timestamp_utc": datetime.now(timezone.utc).isoformat(), "elapsed_seconds": round(tick - start, 3),
                "recording": bool(args.output), "active": True, "agent": args.agent, "model": metadata["model"], "containers": {}}
            current["hardware"] = metadata["hardware"]
            current["performance"] = docker.performance(identities["basalt"])
            if any(m.get("Destination") == "/run/secrets/models.json" for m in identities["basalt"].get("Mounts", [])):
                performance = current["performance"]
                active = performance.get("model") if performance.get("available") and performance.get("state") != "unloaded" else "unloaded / loading"
                if metadata["model"].get("preset") != active:
                    metadata["model"] = model_metadata(identities["basalt"], performance)
                current["model"] = metadata["model"]
            for name, identity in identities.items():
                raw = docker.get("/containers/" + identity["Id"] + "/stats?stream=false&one-shot=true")
                current["containers"][name], previous[name] = normalize(raw, previous.get(name))
                values = current["containers"][name]
                peaks = summary["containers"][name]
                peaks["cpu_max_percent"] = max(peaks["cpu_max_percent"], values["cpu_percent"] or 0)
                peaks["memory_max_bytes"] = max(peaks["memory_max_bytes"], values["memory_working_set_bytes"])
                if args.output:
                    writer.writerow({"timestamp_utc": current["timestamp_utc"], "elapsed_seconds": current["elapsed_seconds"],
                        "container": name, **values})
            top = docker.get("/containers/" + identities["basalt"]["Id"] + "/top?ps_args=-eo%20pid")
            basalt_pids = {row[0].strip() for row in top.get("Processes", [])}
            current["gpu"] = gpu_sample(basalt_pids)
            current["host"], previous_host = host_sample(previous_host)
            if args.require_no_swap and (current["host"] is None or current["host"].get("swap_total_bytes") != 0):
                summary["invalid_reason"] = "host swap is enabled or could not be verified"
                raise RuntimeError(summary["invalid_reason"])
            if args.output:
                samples.write(json.dumps(current, separators=(",", ":")) + "\n")
                performance = current["performance"]
                last_request = performance.get("last") or {}
                performance_writer.writerow({"timestamp_utc": current["timestamp_utc"], "elapsed_seconds": current["elapsed_seconds"],
                    **{k: performance.get(k) for k in ("available", "state", "model", "live_decode_tokens_per_second",
                        "live_prefill_tokens_per_second", "generated_tokens", "active_requests")},
                    **{"last_" + k: last_request.get(k) for k in ("decode_tokens_per_second", "prefill_tokens_per_second", "output_tokens", "ttft_ms")}})
                gpu, host = current["gpu"] or {}, current["host"] or {}
                system_writer.writerow({"timestamp_utc": current["timestamp_utc"], "elapsed_seconds": current["elapsed_seconds"],
                    "host_cpu_percent": host.get("cpu_percent"), "host_memory_available_bytes": host.get("memory_available_bytes"),
                    "host_swap_used_bytes": host.get("swap_used_bytes"), "gpu_utilization_percent": gpu.get("utilization_percent"),
                    "host_swap_total_bytes": host.get("swap_total_bytes"), "host_swap_in_pages": host.get("swap_in_pages"),
                    "host_swap_out_pages": host.get("swap_out_pages"),
                    "gpu_power_watts": gpu.get("power_watts"), "gpu_memory_used_mib": gpu.get("memory_used_mib"),
                    "basalt_gpu_memory_mib": gpu.get("basalt_process_memory_mib")})
            save(args.live / "live.json", current)
            overlay = []
            for name, values in current["containers"].items():
                cpu = values["cpu_percent"]
                cpu_text = "..." if cpu is None else f"{cpu:.1f}%"
                overlay.append(f"{name.upper()}: CPU {cpu_text} | RAM {values['memory_working_set_bytes'] / 2**20:.0f} MiB")
            if current["gpu"]:
                gpu = current["gpu"]
                overlay.append(f"Whole GPU: {gpu['utilization_percent']:.0f}% | {gpu['memory_used_mib']:.0f} MiB | {gpu['power_watts']:.0f} W")
            performance = current["performance"]
            if performance.get("available"):
                rate = performance.get("live_decode_tokens_per_second")
                live = f"{rate:.2f} tok/s" if rate is not None and performance.get("state") == "generating" else "idle / prefill"
                overlay.append(performance["model"] + " | Live decode: " + live)
            temporary = args.live / "overlay.new"
            temporary.write_text("\n".join(overlay) + "\n")
            temporary.replace(args.live / "overlay.txt")
            summary["samples"] += 1
            remaining = args.interval - (time.monotonic() - tick)
            if remaining > 0:
                time.sleep(remaining)
    finally:
        summary["duration_seconds"] = round(time.monotonic() - start, 2)
        if args.output:
            save(args.output / "summary.json", summary)
        for file in files:
            file.close()
        if (args.live / "live.json").exists():
            last = json.loads((args.live / "live.json").read_text())
            last["active"] = False
            save(args.live / "live.json", last)
        (args.live / "recorder.pid").unlink(missing_ok=True)


if __name__ == "__main__":
    main()
