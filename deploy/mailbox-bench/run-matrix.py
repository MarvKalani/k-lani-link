"""Run the fixed seven-carrier decision matrix, without touching production services.

Build once with run.py first. Default CPU affinity matches the measured Ryzen 7600X:
SMT siblings are (0,6), (1,7), ..., (5,11). Use --serial on other hardware.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import threading

ROOT = Path(__file__).resolve().parents[2]
RUNNER = Path(__file__).with_name("run.py")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", action="store_true")
    args = parser.parse_args()
    if not args.serial and not set(range(12)).issubset(os.sched_getaffinity(0)):
        parser.error("The predefined disjoint CPU sets require CPUs 0..11; use --serial.")
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output = ROOT / ".release" / ("https-matrix-" + stamp)
    output.mkdir(mode=0o700, parents=True)
    manifest = {"started_utc": stamp, "planned_cases": 63, "active_seconds_per_case": 60,
                "idle_seconds_when_selected": 65, "parallel_disjoint_cpu_sets": not args.serial, "runs": []}
    mutex = threading.Lock()

    def suite(name, options, server="", client=""):
        command = [sys.executable, "-B", str(RUNNER), "--skip-build", "--seconds", "60", "--rtts", "50", "--profile", "small", *options]
        if not args.serial:
            command += ["--server-cpuset", server, "--client-cpuset", client]
        print("Starting " + name, flush=True)
        log_path = output / (name + ".txt")
        with log_path.open("w") as log:
            subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
        lines = log_path.read_text().splitlines()
        result_path = next(line.removeprefix("Aggregate measurements: ") for line in lines if line.startswith("Aggregate measurements: "))
        with mutex:
            manifest["runs"].append({"suite": name, "directory": result_path})
            (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print("Completed " + name, flush=True)

    def proxies():
        suite("upload-buffered", ["--tls", "nginx-upload-buffered", "--workloads", "100:10", "--drain-seconds", "30"], "4,10", "5,11")
        suite("response-buffered", ["--tls", "nginx-response-buffered", "--workloads", "100:10", "--drain-seconds", "30"], "4,10", "5,11")

    def one():
        suite("baseline-one", ["--workloads", "50:1,100:1"], "0,6", "1,7")
        suite("idle-cut", ["--tls", "nginx", "--idle-seconds", "65", "--workloads", "100:1"], "0,6", "1,7")

    def ten(proxy_future=None):
        suite("baseline-ten", ["--workloads", "50:10,100:10"], "2,8", "3,9")
        suite("receiver-cut", ["--workloads", "100:10", "--cut-after-records", "400"], "2,8", "3,9")
        if proxy_future is not None:
            proxy_future.result()  # The large server borrows a core only after that lane is done.
        suite("large", ["--profile", "large", "--workloads", "100:10"], "2,3,8,9", "4,10")

    if args.serial:
        proxies()
        one()
        ten()
    else:
        with ThreadPoolExecutor(max_workers=3) as pool:
            proxy_future = pool.submit(proxies)
            futures = [proxy_future, pool.submit(one), pool.submit(ten, proxy_future)]
            for future in futures:
                future.result()
    manifest["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print("Completed decision matrix: " + str(output), flush=True)


if __name__ == "__main__":
    main()
