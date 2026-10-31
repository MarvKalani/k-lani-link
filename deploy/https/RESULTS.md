# Actual HTTPS application-path results — 29 October 2026

**Decision: keep the selected mailbox. The response flow-control bottleneck found in the
first run is fixed without a contract, window or wire change; all four workloads now
complete live.** This is still a private source preview: no release, public relay or
default-transport change. No eighth carrier, TURN service, WebRTC change or protocol
shortcut was introduced, and the proposed 32-chunk HTTPS window amendment is withdrawn.

[Corrected-run evidence](results-2026-10-29-coalesced.json) ·
[First-run evidence](results-2026-10-29.json) ·
[Reproduce](README.md#reproduce-the-bounded-docker-measurements) ·
[Original seven-carrier decision](../mailbox-bench/DECISION.md)

## Measured setup

- COTS host: AMD Ryzen 5 7600X, Ubuntu 26.04.1 LTS, Linux 7.0.0-38-generic (first run) /
  7.0.0-42-generic (corrected run),
  Docker Engine 29.8.1. This is not the owner's EPYC VPS or an Internet route.
- Two scratch containers: the actual native-TLS relay; a synthetic backend plus the
  actual host, gateway and independent endpoint HTTP pools in the load container.
- Relay: 2 CPU quota / 1536 MiB, matching the earlier small-VPS budget. Load: 4 CPU
  quota / 512 MiB. No LLM/GPU or real tokenization; each synthetic SSE token is 161 bytes.
- Internal Docker network, no published ports, no container logs, core dumps or swap;
  read-only root filesystems, UID 1000, no capabilities. The trusted seccomp allowlist
  was narrowed to stream sockets. IPv4/IPv6 UDP refusal, including NONBLOCK sockets,
  was tested before the workloads. Unreachable environment proxies were ignored.
- Shaping: 25 ms on each endpoint/relay egress = 50 ms endpoint-to-relay RTT;
  500 Mbit/s egress per container. TLS verification remained enabled with a disposable
  private CA and a separate valid server certificate.
- Exactly the four fixed 60-second workloads, once before and once after the correction.
  Each stream had a 30-second additional drain deadline. No automatic application retry.
  All test containers/networks and private endpoint/certificate material were removed.
- **Corrected run only:** host swap was enabled, so it used a scratch copy of
  `run-lab.py` with the swap gate disabled (and the repository root fixed); every
  container still had `--memory-swap` equal to `--memory`, so none could swap. The JSON
  records this. Rerun with host swap off before quoting these figures as release evidence.

## Completion and latency

Latency starts at each scheduled synthetic token time and includes backend scheduling,
application flow control, batching and transport. First token includes connection setup.
Percentiles in failed rows describe only the received prefix, not all offered tokens.
Successful completion requires every event in order, `[DONE]` and the authenticated
gateway stream ending normally. Wall time near 60 s shows that no backlog builds up.

**Corrected run** (source `542a6f5`: coalesced response chunks, per-direction relay
wakeups, jittered retry backoff):

| Tokens/s per tunnel | Tunnels | Received / offered | Completed tunnels | First token, slowest | Token delay p50 | p99 | Wall time incl. setup/drain |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 50 | 1 | 3,000 / 3,000 | 1/1 | 862 ms | 125 ms | 166 ms | 60.75 s |
| 100 | 1 | 6,000 / 6,000 | 1/1 | 838 ms | 93 ms | 158 ms | 60.69 s |
| 50 | 10 | 30,000 / 30,000 | 10/10 | 988 ms | 125 ms | 161 ms | 60.84 s |
| 100 | 10 | 60,000 / 60,000 | 10/10 | 989 ms | 106 ms | 168 ms | 60.86 s |

**First run** (source `ab59bf6`), kept for comparison:

| Tokens/s per tunnel | Tunnels | Received / offered | Completed tunnels | First token, slowest | Token delay p50 | p99 | Wall time incl. setup/drain |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 50 | 1 | 3,000 / 3,000 | 1/1 | 838 ms | 647 ms | 1,093 ms | 61.75 s |
| 100 | 1 | 4,394 / 6,000 | 0/1 | 843 ms | 23,055 ms | 45,410 ms | 90.43 s |
| 50 | 10 | 30,000 / 30,000 | 10/10 | 1,012 ms | 570 ms | 997 ms | 61.82 s |
| 100 | 10 | 44,052 / 60,000 | 0/10 | 990 ms | 22,979 ms | 45,321 ms | 90.58 s |

The first run's 50-token/s rows already ran at the ceiling (about 49 events/s), which
explains their 0.6 s median delay. The earlier carrier-only 96 ms / 130 ms median/p99
remain a carrier measurement, not this application's latency.

**Padding check** (source `3db991a`, records padded to 1/2/4/8/12 KiB; same scratch
runner with host swap on, and a workspace test run overlapped part of it, so treat it as a
regression check, not release evidence): all four workloads complete, 3,000 / 6,000 /
30,000 / 60,000 tokens, p50 124 / 127 / 124 / 105 ms, p99 ≤ 169 ms, wall ≤ 60.87 s; relay
CPU ≤ 2.57 %, relay RAM ≤ 6.9 MiB. The 100 × 1 median rose from 93 to 127 ms; with one
run each, that is not separable from noise.

**HTTP/1.1 versus HTTP/2** (branch `http2-mailbox`, run back to back with the same
scratch runner, host swap on; another session's Docker build may have overlapped):

| Tokens/s × tunnels | p50 h1 / h2 | p99 h1 / h2 | First token h1 / h2 | Relay CPU h1 / h2 | Load CPU h1 / h2 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 50 × 1 | 123 / 123 ms | 169 / 169 ms | 850 / 644 ms | 0.53 / 0.64 % | 1.54 / 1.74 % |
| 100 × 1 | 93 / 93 ms | 159 / 128 ms | 839 / 647 ms | 0.43 / 0.69 % | 1.37 / 1.82 % |
| 50 × 10 | 124 / 124 ms | 163 / 162 ms | 1,030 / 800 ms | 2.65 / 3.41 % | 8.55 / 9.95 % |
| 100 × 10 | 93 / 103 ms | 163 / 167 ms | 1,014 / 803 ms | 2.46 / 3.48 % | 8.95 / 10.59 % |

All 3,000 / 6,000 / 30,000 / 60,000 tokens arrived in both. HTTP/2 does not raise
throughput or lower CPU here; it saves connection setup (about 200 ms to the first token)
and uses one connection per endpoint, which matters for proxies that limit connections.
Its framing costs slightly more CPU. One run each; the p50/p99 differences are within noise.

**HTTP/2 versus HTTP/3 (QUIC), with and without 1 % packet loss** (branch
`http2-mailbox`, 100 tokens/s, run back to back, nothing else running; host swap on;
loss is netem on each container's egress, so both directions):

| Variant | Tunnels | p50 | p99 | First token, slowest | Relay CPU | Load CPU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| HTTP/2, no loss | 1 / 10 | 105 / 104 ms | 167 / 167 ms | 643 / 797 ms | 0.60 / 3.29 % | 1.71 / 10.32 % |
| HTTP/3, no loss | 1 / 10 | 128 / 94 ms | 169 / 159 ms | 652 / 798 ms | 1.06 / 5.54 % | 1.62 / 10.44 % |
| HTTP/2, 1 % loss | 1 / 10 | 113 / 110 ms | 390 / 415 ms | 713 / 1,952 ms | 0.48 / 2.48 % | 1.46 / 8.22 % |
| HTTP/3, 1 % loss | 1 / 10 | 101 / 99 ms | 300 / 301 ms | 650 / 813 ms | 0.86 / 4.52 % | 1.42 / 8.99 % |

Every run delivered all tokens. Without loss HTTP/3 is not faster (differences are within
one-run noise) and costs the relay more CPU (about 1.7×, still below 6 % of one CPU at
10 tunnels). With 1 % loss HTTP/3 cuts the p99 by about a quarter (300 vs 390–415 ms) and
keeps connection setup fast under load (813 vs 1,952 ms to the slowest first token), as
expected from QUIC's per-stream loss recovery. The planned "HTTP/3 with UDP blocked" run
did not start: the lab's seccomp UDP block also stops the relay's own QUIC socket. The TCP
fallback is covered by the transport tests, not measured here.

## Resources of both containers

CPU is cgroup CPU time / measured wall time; 100% means one CPU. RAM is the cgroup
high-water mark, including runtime allocation/page cache. Helpers used separate cgroups.
The load figure includes both endpoint roles and the synthetic backend, not OpenCode
or Basalt GPU inference. No RAM or CPU saturation was observed. Corrected run (first run
in parentheses):

| Tokens/s × tunnels | Relay CPU | Load CPU | Relay peak RAM | Load peak RAM |
| --- | ---: | ---: | ---: | ---: |
| 50 × 1 | 0.45% (0.33%) | 1.37% (0.90%) | 5.42 MiB (5.15) | 5.44 MiB (5.43) |
| 100 × 1 | 0.42% (0.20%) | 1.52% (0.59%) | 5.44 MiB (5.20) | 5.66 MiB (4.89) |
| 50 × 10 | 2.43% (2.93%) | 7.56% (5.18%) | 7.01 MiB (7.01) | 11.88 MiB (12.18) |
| 100 × 10 | 2.27% (1.94%) | 8.12% (3.58%) | 6.24 MiB (5.95) | 11.64 MiB (19.86) |

The first run's 100-token/s rows delivered fewer tokens, so their lower CPU is not a
like-for-like comparison. The laboratory image is 39,305,053 uncompressed bytes; the
separately built relay image contains the main executable and notices. Neither image
includes a userspace operating-system distribution. Image bytes are Docker-reported
sizes, not download sizes or steady RAM usage.

## Finding and correction

The application grants **eight response chunks** per request; each consumed chunk
returns a Credit frame. The host turned every upstream body item into its own chunk,
and an OpenAI-compatible SSE backend writes one small event per item. Each 161-byte
event therefore needed its own credit round trip: about 2 × (30 ms batching + 50 ms
relay path) ≈ 160 ms, so 8 credits / 0.16 s ≈ 50 events/s — the observed ~49.
The window size was not the real limit; spending a whole credit on 161 bytes was.

**Correction (`src/host.rs`):** while no credit is free, the host appends further
upstream items to the next chunk, up to the existing 8 KiB chunk size; with a free
credit nothing waits. Throughput is now limited by bytes per round trip (8 × 8 KiB),
not by events. Window, chunk size, frames, batching, retention and crypto are unchanged;
WebRTC uses the same code. Buffered response data per request is bounded by one chunk
plus one upstream item. The previously proposed 32-chunk HTTPS-only window is
**withdrawn**: it would have forked the two transports and still capped near 200 events/s.

Regression tests cover coalescing under withheld credits (byte order preserved, no chunk
above 8 KiB, far fewer chunks than events) and that one full 8 KiB data frame fits a
sealed batch. Two shared-relay issues found in the same review were fixed alongside:
a relay-wide wakeup on every stored record (now per session direction or handshake room,
with expiry scoped to the addressed room) and fixed 100 ms client retries (now jittered
exponential backoff up to 2 s, honoring a bounded `Retry-After`). This lab cannot show
their scaling effect at ten tunnels; it shows no regression.

## Correctness evidence and remaining limits

The Rust tests separately exercise native TLS, Basic CONNECT authentication, rejection
of untrusted certificates/redirects, finite-body buffering, exact ciphertext retry after
a lost response, resume after a truncated GET, thinking/tool SSE, two parallel requests
and cancellation. Tampering, duplicate/reordered records and replayed replies fail closed.
An actual relay restart ends old sessions and fails their requests; the gateway then
negotiates a new session by itself without replaying work (unit-tested; see below). The existing S1–S3 child-process crash tests remain separate
state evidence; they do not restore live streams after an endpoint crash.

The intermittent five-second WebRTC setup timeout in `tests/multiplex.rs` (also present
before the HTTPS work) is fixed without raising a deadline: webrtc 0.21 silently dropped
the loopback candidate, so local tests ran ICE over about 20 Docker/VM bridge addresses
(400 pairs per peer); with four labs in parallel the kernel dropped about 900 STUN
datagrams per run and setup took up to 8 s. `include_loopback_candidate` now binds only
`127.0.0.1`. Afterwards 25 of 25 parallel `multiplex` runs and three full workspace runs
passed (before: 9 of 12 `multiplex` runs failed).

This is not full shared-use saturation or a hostile-user capacity audit. Real corporate
WAN/proxies, NTLM/Kerberos/PAC, Windows/macOS HTTPS bundle UX, a complete process-level
endpoint restart exercise through the new CLI, and real Basalt inference over HTTPS
remain unverified. The large VPS budget was not rerun. Real backends may write several
events per body item; the correction only removes per-item credit cost, it does not
assume one layout. Gateway auto-reconnect after a lost HTTPS session was added after
this measurement and is covered by tests only, not by this lab: in-flight requests fail,
the backend request is dropped, a fresh session follows, and automatic retries stop
before lost replies could use up the host's pending attempts. No public relay was installed,
and existing STUN, signaling, WebRTC and office-model deployments were left alone.
