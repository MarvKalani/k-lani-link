# Carrier experiment results — 28 October 2026

**Historical v1 experiment.** For current carrier selection, use the [63-case decision matrix](DECISION.md). Its mailbox uses non-destructive multi-record reads and acknowledgement; the old serial destructive mailbox below is not an additional current variant.

42 completed cases; this is a short, synthetic local experiment, **not** an EPYC, Internet,
LLM or TURN benchmark. The [aggregate JSON](results-2026-10-28.json) contains every included
result, environment metadata and measured source hashes. Failed/incomplete development
runs and the initial runs before enabling TCP_NODELAY are excluded.

## Environment

- Host: AMD Ryzen 5 7600X, Ubuntu 26.04.1 LTS, Linux 7.0.0-38-generic, Docker 29.8.1.
- Test containers: Debian 13, swap off, read-only roots, no published ports, private internal
  network. One relay container and one load generator holding both logical endpoints.
- Small relay budget: 2 CPUs and 1,536 MiB RAM, reserving OS headroom relative to a
  small 2 vCPU VPS (1.82 GiB total RAM).
- Large relay budget: 4 CPUs and 7,168 MiB RAM, based on a typical 4 vCPU/8 GB VPS
  plan. This is not confirmation of the owner's exact large-server contract.
- Each container's egress: 500 Mbit/s; half the requested RTT added on each side. Measured
  HTTPS median RTT was approximately 0.1–0.2 / 50.3–50.4 / 150.3–150.4 ms in direct mode.
- HTTPS HTTP/1.1, verified certificates, keep-alive, TCP_NODELAY, AEAD on every record,
  up to 30 ms batching. One synthetic token = one 161-byte SSE event. No model was loaded.

CPU figures below are approximate cgroup CPU time divided by wall time; 100% means one
fully occupied CPU, not the entire machine. Peak RAM is cgroup memory, not just process
RSS. The relay budget includes Nginx in the Nginx comparison. The experiment does not
measure load-generator resource consumption or peak live connection counts.

## Same 1,000-token/s workload, small budget, native Rust TLS

Each run offers 6,000 synthetic tokens over six seconds. Token delay starts at the
scheduled generation time and ends at verified decryption, including batching and initial
connection setup. p99 in a failed run describes only its delivered prefix, not all input.

| Endpoint↔relay RTT | Carrier | Received / offered | Failed tunnels | p50 delay | p99 delay | CPU (% of one CPU) | Peak RAM |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 ms | mailbox | 6,000 / 6,000 | 0 | 17 ms | 32 ms | 0.45% | 18.9 MiB |
| 0 ms | continuous stream | 6,000 / 6,000 | 0 | 17 ms | 32 ms | 0.24% | 13.6 MiB |
| 50 ms | mailbox | 1,410 / 6,000 | 1 | 674 ms | 1,140 ms | 0.52% | 19.3 MiB |
| 50 ms | continuous stream | 6,000 / 6,000 | 0 | 67 ms | 141 ms | 0.29% | 14.6 MiB |
| 150 ms | mailbox | 240 / 6,000 | 1 | 964 ms | 1,352 ms | 0.55% | 18.9 MiB |
| 150 ms | continuous stream | 6,000 / 6,000 | 0 | 169 ms | 556 ms | 0.29% | 12.6 MiB |

The mailbox reaches its 32-record unread cap while serial GETs consume too slowly. The
three failed PUT attempts end that tunnel. Large resources produce the same received
counts (6,000 / 1,410 / 240) for the mailbox at the three RTTs, while the stream delivers
all 6,000 at every RTT. This is a receive-protocol bottleneck in these tests, not CPU/RAM
or link saturation. It is not a fundamental limitation of HTTPS or GET.

The direct stream's measured relay egress was around 1.8 Mbit/s for 1,000 synthetic tokens/s.
The 500 Mbit/s cap was therefore nowhere near saturated. A different SSE event size or a
large prompt changes traffic; token rate alone is not a complete capacity specification.

## Lower rates, parallel streams and repeated runs

- At 50 tokens/s and 50 ms RTT, the six-second mailbox run eventually delivers all 300
  tokens, but its p99 delay reaches 1.71 seconds. The stream delivers them at 91 ms p99.
  A short run completing does not mean its receive rate can sustain an indefinite stream.
- Ten tunnels at 100 tokens/s each fail in the mailbox at 50 and 150 ms RTT. All ten stream
  tunnels deliver their full output. Aggregate offered rate stays 1,000 tokens/s.
- 100 tunnels at ten tokens/s each complete for both carriers in the six-second cases.
  At 150 ms RTT the mailbox p99 is 3.54 seconds versus 458 ms for the stream. Across these
  100-tunnel stream runs, relay peak RAM is 11.9–15.2 MiB and average CPU 2.2–3.3% of one CPU.
  These are short-run observations, not safe tenant or long-running connection limits.
- Three ten-second repetitions at 1,000 tokens/s and 50 ms RTT reproduce the distinction:
  each mailbox run delivers 1,410/10,000 and aborts; every stream delivers 10,000/10,000.
  Stream median delay is 66.7–67.6 ms and p99 is 101.2–101.3 ms. The six-second p99 differs
  because initial TLS setup affects a larger fraction of its samples.
- With Nginx 1.26.3, buffering disabled, the 1,000-token/s tests have the same completion
  pattern. Stream p99 is approximately 32 / 132 / 542 ms at 0 / 50 / 150 ms RTT. Nginx is
  unnecessary for this relay and does not solve the serial-GET bottleneck.

## Reproduce and interpret

```sh
python3 deploy/mailbox-bench/run.py --profile small --seconds 6
python3 deploy/mailbox-bench/run.py --skip-build --profile large --workloads 1000:1 --seconds 6
python3 deploy/mailbox-bench/run.py --skip-build --profile small --tls nginx --workloads 1000:1 --seconds 6
python3 deploy/mailbox-bench/run.py --skip-build --profile small --rtts 50 --workloads 1000:1 --seconds 10 --repeat 3
```

The 20 ms PUT retry delay is an explicit experiment choice because the draft did not
specify it. Different backpressure/retry policies affect where failure occurs; they do
not remove the serial receiver's RTT ceiling. The stream uses bounded channel backpressure
and is not a complete reconnect or message-recovery protocol.

Not verified: actual EPYC/WAN performance, jitter/loss and response-loss recovery, corporate
proxy buffering/inspection, handshake persistence, hostile-box behavior in a finished
transport, full duplex credits, bulk prompts and tool results, real model output, 1,000
concurrent tunnels, sustained saturation, and direct-WebRTC/TURN comparisons. A production
decision still needs those measurements after the correctness defects in the
[review](../../docs/MAILBOX-REVIEW.md) are resolved.
