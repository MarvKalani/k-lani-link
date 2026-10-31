# Reference machine and measurement scope

Observed on 28 October 2026. These are commodity components, not minimum system
requirements or a performance guarantee.

| Component | Observed configuration |
| --- | --- |
| CPU | AMD Ryzen 5 7600X, 6 cores / 12 threads |
| Motherboard | ASUS TUF GAMING B650-PLUS WIFI |
| Installed RAM | 2 × 16 GiB G.Skill DDR5, F5-6000J3038F16G, configured 6000 MT/s |
| GPU | NVIDIA GeForce RTX 5070 Ti, 16,303 MiB visible VRAM |
| GPU driver / power limit | 595.99.02 / 300 W |
| Storage | 2 × Samsung SSD 990 PRO with Heatsink, 2 TB each |
| Model filesystem | ext4 on `/dev/md0`, RAID 0 from two loopfiles on separate NVMe devices |
| Host OS / kernel | Ubuntu 26.04.1 LTS / Linux 7.0.0-38-generic |
| Docker / Compose | Engine 29.8.1 / Compose 5.5.1 |
| Basalt container | Ubuntu 24.04.4 LTS, CUDA 13.2.0, network=none |
| OpenCode container | Debian 13 trixie, OpenCode 2.2.0, network=none |
| ThinkRail alternative | 0.3.0-nightly.7 with Pi 1.4.0 |
| Document tools | WordCraft 0.11.0 / GridCraft 0.12.1 |
| Tested model ID | Qwen3.8-Flash-Next IQ3_XXS |
| Local pack filename | Qwen3.8-FN-GSQ-RCO-Coder.basalt |
| Pack size | 60,829,697,334 bytes, approximately 56.65 GiB |
| Context / KV | 262,144 tokens, one slot, int8 KV |
| Bounded-memory demo | Basalt 4 GiB RAM, mmap experts, prefill 512; OpenCode 1 GiB RAM |
| Swap | Host disabled; container memory and memory+swap limits equal |

The pack SHA-256 is
`59fd8ca95adc63cc52e4a1ca4d16df579f89a184a2ab80ba5ec3a97e6987169b`.
Do not infer a pack's measured quality from its local filename. The alternate
NVFP4 file was not used for this recording. The reported 14.3 GB/s RAID throughput
was not remeasured here. Sequential disk bandwidth, file-cache behavior and
end-to-end inference throughput are distinct measurements.

## What the dashboard records

CPU uses 100% for one logical CPU. Container working-set RAM is usage minus
inactive file pages. GPU power/utilization cover the whole GPU, including other
programs; only process VRAM is attributed to Basalt. Docker network counters
exclude Unix-socket traffic. Disk counters are cumulative from container start.
No claim about total wall-socket energy follows from GPU power alone.

The optional numeric exporter reports real engine token counts, including
generated reasoning and tool arguments. The dashboard separates live decode
(a two-second window), last-request decode, prefill and first-token latency.
It never estimates tokens by counting characters or SSE chunks. Missing/stale
measurements appear as unavailable. Hardware metadata excludes serial numbers,
machine names, UUIDs and network addresses. RAM-module details are an optional
operator-supplied allowlisted snapshot; the collector never runs sudo.

The English business-demo recording predates this numeric exporter and therefore
contains resource counters, not measured token/s. Earlier 43–46 tok/s figures
from a different memory profile are not assigned to that recording. Swap is
verified at each sample; enabling it invalidates the measurement. Files produced
by an agent and its local history still cause normal disk writes.

## Tunnel latency

200 complete `GET /v1/models` round trips after 10 warm-ups measured **0.780 ms
median, 1.060 ms p95** (0.625–1.290 ms). Both peers were on the same host.
This includes API, encryption, transport and backend overhead; it is not pure
network RTT, time to first token, or a company-to-home WAN measurement.
Reproduce with [measure-latency.py](measure-latency.py) on the actual route.

## Short measured request (28 October 2026)

Raw values: [throughput-sample.json](throughput-sample.json).

A synthetic request through the existing encrypted same-host tunnel generated
62 tokens from a 69-token prompt. Basalt's DONE counters reported **7.9 token/s
decode**, 9.9 token/s prefill (6,990.3 ms), and 7,867.2 ms decode. The client
received its first reasoning/content delta after **7.046 seconds** and completed
in 14.865 seconds. Both reasoning and answer content arrived; usage reported
62 completion tokens, matching the numeric exporter. SSE event count was 58,
which illustrates why events are not a valid token count.

This is a single short request with the bounded-memory profile above, not a
sustained throughput or full-context benchmark. Earlier attempts hit Basalt's
5,632 MiB host-RAM guard; the guard and swap policy were not bypassed. The
unused ThinkRail alternative was stopped during this OpenCode measurement.

## Managed-model validation (28 October 2026)

[Recorded functional results](managed/validation-20261028.json) cover Coder →
NVFP4 → Coder over one authenticated tunnel, followed by unloading. Both packs
were on the same RAID 0 filesystem. Each model supplied two active slots with
262,144-token capacity per slot, INT8 KV and prefill 512. Six pairs of short
requests completed with 128 generated tokens each, including reasoning. The
signaling container stayed stopped; no new pairing code was used for the changes.

The backend used a **6 GiB memory limit with swap disabled**. A preceding 4 GiB
attempt had a kernel-confirmed backend OOM during NVFP4; it is not counted as a
successful run. The 6 GiB run had zero OOM events. This does not guarantee that
longer prompts or two filled contexts fit the same budget. Request durations are
end-to-end and include prefill and scheduling; they are not engine-only decode
rates. The GPU power limit was 300 W; a desktop service occupied about 260 MiB of
VRAM. Host/container resource samples remain in the private test record.
