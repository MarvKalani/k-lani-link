# Seven-carrier HTTPS decision experiment

[Decision table and measured evidence](DECISION.md) · [Delivery proposal awaiting review](../../docs/DESIGN-MAILBOX.md)

This is a separate benchmark, **not a production transport**. It changes no host/gateway,
WebRTC, pairing lease, or persistent key-state implementation. Never expose this test
server to the Internet: its bearer value is public synthetic data. Production implementation
must wait for the owner's review of the decision table and proposed delivery contract.

The original [42 short tests](RESULTS.md) are historical evidence for the previous serial,
destructive mailbox. The new mailbox below replaces that implementation in the experiment;
it is not an eighth mode. The new matrix compares exactly these seven modes:

| Mode | Upload | Receive | Retention |
| --- | --- | --- | --- |
| `mailbox` | Finite PUTs | Multi-record GET `?from=N`, bounded long poll | Non-destructive; next cursor acknowledges earlier records |
| `stream` | Continuous HTTP/1.1 PUT body | Continuous binary GET `?from=N` | Retained until explicit ACK or expiry |
| `post-stream` | Finite POSTs | Continuous binary GET `?from=N` | Retained until explicit ACK or expiry |
| `sse` | Finite POSTs | SSE GET `?from=N`, ciphertext in base64 | Retained until explicit ACK or expiry |
| `longpoll` | Finite POSTs | Multi-record long poll `?from=N` | Destructive reference for the lost-response test |
| `h2-stream` | Continuous PUT on HTTP/2 | Binary GET on HTTP/2 `?from=N` | Retained until explicit ACK or expiry |
| `wss` | Binary WebSocket over TLS | Binary WebSocket, upgrade URL `?from=N` | Retained until explicit ACK or expiry |

HTTP/2 is verified on actual responses; it is not inferred from a configuration flag.
This is native HTTP/2 streaming, not a gRPC framework benchmark. WSS uses the HTTP/1.1
Upgrade protocol and is listed distinctly from ordinary HTTP requests. Both logical
endpoints have independent connection pools; HTTP/2 does not share one connection between
sender and receiver. Each tunnel also has its own endpoint pools. This differs from v1,
so compare rows **within the new matrix**, not old and new CPU figures as if identical.

## Run the fixed matrix

Linux Docker Engine with cgroup v2 and `netem`, Rust/Cargo, Python 3 and OpenSSL are required.
Host swap must be disabled; the runner checks it and never changes it. Build dependencies
can access the Internet. Runtime containers have an internal network and no published ports.
No production keys, workspace, model files or Docker socket are mounted.

Build and smoke-test once from the repository root:

```sh
python3 -B deploy/mailbox-bench/run.py --profile small --rtts 50 --workloads 100:1 --seconds 3
```

Then run the fixed decision matrix:

```sh
python3 -B deploy/mailbox-bench/run-matrix.py --serial
```

The matrix has 63 cases, each with **60 seconds of active generation**. The idle cases add
65 seconds of deliberate silence halfway through, giving about 125 seconds plus startup
and drain. Every carrier is included in each suite:

| Suite | Load per tunnel | Tunnels | Proxy / fault | Cases |
| --- | --- | --- | --- | ---: |
| Baseline | 50 and 100 synthetic tokens/s | 1 and 10 | Native Rust TLS, 50 ms RTT | 28 |
| Upload buffering | 100 tokens/s | 10 | Nginx buffers the request body | 7 |
| Response buffering | 100 tokens/s | 10 | Nginx buffers the response | 7 |
| Receiver cut | 100 tokens/s | 10 | Drop an unverified record after 400 verified records, reconnect with the same cursor | 7 |
| Idle cut | 100 tokens/s | 1 | Nginx has 60 s idle timeouts; generation deliberately pauses for 65 s | 7 |
| Larger budget | 100 tokens/s | 10 | Native Rust TLS, larger CPU/RAM allowance | 7 |

All suites use 50 ms endpoint-to-relay RTT and 500 Mbit/s per-container egress shaping.
This keeps the requested workload matrix comparable and bounded. No jitter/loss is injected
in this fixed matrix; `run.py` exposes those parameters for separately identified experiments.
The earlier high-rate, 0/50/150 ms results are retained as historical measurements.

On the measured Ryzen 5 7600X only, `run-matrix.py` without `--serial` uses three independent
lanes on disjoint physical cores: relay/client CPU sets `0,6` / `1,7`, `2,8` / `3,9`, and
`4,10` / `5,11`. SMT siblings are paired. The larger-budget case waits until the proxy lane
finishes, then uses server `2,3,8,9` and client `4,10`. This shortens wall time but does not
turn one physical machine into independent machines: RAM, kernel and host scheduling are
still shared. CPU sets and all test options are recorded. Do not reuse this affinity map
blindly on a different CPU topology; use the serial option instead.

## Resource profiles and isolation

| Profile | Relay CPU quota | Relay RAM limit | Reference |
| --- | ---: | ---: | --- |
| small | 2 | 1,536 MiB | A small 2 vCPU VPS with 1.82 GiB total RAM |
| large | 4 | 7,168 MiB | A typical 4 vCPU / 8 GB VPS plan |

RAM budgets leave guest-OS headroom. The exact large-server contract was not confirmed.
Resource quotas do not emulate EPYC instructions, noisy neighbours or an Internet route.
The provisional interpretation of the owner's network measurement is 500 **Mbit/s**.
The load-generator limit is 4 CPUs / 512 MiB; the affinity map further restricts it to two
logical CPUs. The host CPU, OS, kernel, Docker version, image ID, source hashes and settings
are recorded for each run. No production VPS is modified or loaded by these commands.

Nginx is optional. Native mode runs the Rust server with Rustls. The instrumentation image
contains Nginx and diagnostic tools for comparison; its size is not a minimum production
image. A future static binary can use `scratch`, still relying on the host kernel.

Root filesystems are read-only; `/tmp` is a 32 MiB tmpfs. Logs and core dumps are disabled;
container swap is forbidden. The entrypoint temporarily has capabilities to shape **its own**
network namespace, then drops to UID 1000 with no capabilities. Production does not need
these shaping privileges. The client receives only the temporary CA certificate, not the
server's private TLS key. TLS verification stays enabled. Nginx response spill files are
disabled; any request-buffer spill uses tmpfs, never a persistent payload volume.

## What the retention and resume test actually proves

All data uses the existing `SecureSession` AEAD. One synthetic token is a 161-byte SSE event
with eight content characters; no tokenizer or LLM runs. Both carriers and content are
encrypted. Batches use the same 30 ms timer and protocol packet-size caps.

The relay indexes the existing authenticated record header (which is already visible).
It stores up to 32 unacknowledged records per room/direction, with a 16 MiB global payload
cap and 128-room cap. Records expire after 30 s; a one-second sweep removes expired bytes.
A request for a missing expired/acknowledged prefix returns 410 instead of skipping ahead.
Only contiguous records from the requested cursor are returned, up to the queue limit.

In `mailbox`, GET does not delete returned bytes. If a response contains records 20–27,
`from=28` acknowledges everything below 28; repeating `from=20` before that acknowledgement
returns the same retained records. A cursor cannot acknowledge beyond the range offered.
The destructive `longpoll` mode deliberately demonstrates the different failure behavior.

Continuous readers acknowledge every eight verified records through a finite POST; all
modes send a final ACK. This is retention metadata for the experiment, not proof of business
execution. A bounded cache of 64 ciphertext hashes accepts identical retries after ACK and
rejects changed bytes under the same index. Older retries outside that cache fail closed.
Continuous upload handlers wait for capacity; finite uploads use at most eight concurrent
requests and three identical attempts, with 20 ms between retries. These are explicit
prototype choices, not a production backpressure contract.

The injected receiver cut discards an already transported but **unverified** record, closes
the receive transport and reconnects with `?from=N` for the next unverified record. The same
in-memory AEAD session and counter are retained. Invalid authentication/order or an explicit
410 ends the session; it does not trigger a fresh handshake or an automatic backend retry.
The test verifies all delivered records and expects the authenticated End record before
calling a tunnel complete. No duplicate decrypted record is accepted.

Only the **reader** reconnects automatically in this prototype. Continuous PUT and WSS
writers do not yet have an outbound replay journal/reconnect loop. Idle-cut failures in
those writers must be identified as this implementation gap, not an inherent inability of
HTTP/2 or WebSocket to reconnect. No inference request is replayed. Heartbeats are deliberately
disabled so the proxy's idle timeout can fire. Normal long polls return after 25 s and thus
may keep an HTTP connection active without ever hitting that idle timeout.

This does not test endpoint-process restart, durable stream cursors, the buggy persistent
key chain, or a malicious relay. Handshake keys are created in process and never negotiated
through the measured carrier. The later state-module change fixes the three original defects with regression/crash
tests. That change does not wire a production handshake into this experiment or the app.

## Read the results

Only aggregate JSON is written under `.release/mailbox-bench-<UTC>-<id>/`. No payload, room,
key or peer trace is recorded. Matrix manifests collect those directories. Metrics include:

- Offered/received tokens, authenticated completion, fixed failure categories.
- Maximum time to first verified token across tunnels, token delay p50/p99.
- Reader reconnections, injected cuts, and time from reconnect initiation to the next data.
- Upload/GET/ACK request counts, relay interface bytes, CPU and peak RAM for **both** containers.

Token delay starts at its scheduled generation time and includes batching, setup, queueing,
backpressure and transport. Intentional idle time is added to the generation schedule and
is not counted as token transit delay. Reconnect-to-data time can include a remaining idle
period. Throughput uses wall time including setup, intentional idle and drain. Failed-row
percentiles cover only the delivered prefix. Completion after 60 s of buffering does not
qualify as live streaming merely because all tokens eventually arrive.

CPU is cgroup CPU time divided by wall time; 100% means one CPU. Peak memory is cgroup memory,
including shaping/readiness helpers and page cache, not just Rust RSS. Counts named
`put_requests` normalize upload requests (including POST/WS); `rejected_puts` also includes
capacity waits in continuous handlers. These names are retained for v1 JSON compatibility.

## Draft-state counterexamples

```sh
python3 -B deploy/mailbox-bench/reproduce.py
```

It runs `tests/mailbox_state.rs`: the three historical counterexamples are now regressions
asserting the corrected behavior. The old broken-behavior source is available in Git at
`0d92e20`; it is no longer injected into current tests. See the
[review](../../docs/MAILBOX-REVIEW.md) and [state machine](../../docs/DESIGN-MAILBOX.md).
Box/transport/configuration wiring still awaits owner review.
