# Seven-carrier decision matrix — 28 October 2026

63 completed **test cases**, including intentionally failing cases. Every case uses 60 seconds of active synthetic generation; idle cases add 65 seconds of silence.

These are local Docker measurements on a Ryzen 5 7600X, with small-VPS resource budgets, 50 ms shaped RTT and 500 Mbit/s egress. They are not EPYC, Internet, real-model, full-duplex or production-capacity measurements.

[Full aggregate evidence](decision-results-2026-10-28.json) · [Method and reproduction](README.md)

## Decision proposed for owner review

Recommend the **acknowledged multi-record mailbox** as the first ordinary-HTTPS candidate. It completed every tested condition, with 96 ms median token delay versus roughly 71–72 ms for continuous readers at the selected 50 ms RTT. Do not implement production transport before owner review and the three state fixes.

| Carrier | Proposed disposition |
| --- | --- |
| `mailbox` | Initial candidate: finite HTTP, acknowledged retention, all tested conditions complete |
| `stream` | Defer: buffered upload postpones output; response tail/idle writer failures |
| `post-stream` | Possible later optimization where response streaming is verified |
| `sse` | Defer unless browser integration warrants its extra encoding |
| `longpoll` | Reject destructive consumption; use the acknowledged mailbox rule |
| `h2-stream` | Defer continuous upload; successful read-resume does not solve upload buffering/writer reconnect |
| `wss` | Outside the ordinary-HTTP requirement; writer reconnect is also missing here |

The [proposed delivery contract and three state tasks](../../docs/DESIGN-MAILBOX.md) are awaiting owner review. A later optimization is not authorization to build another production carrier.

## Completion and failure behavior

Baseline includes all four requested loads: 50/100 tokens/s per tunnel, with 1/10 tunnels. Proxy tests use ten tunnels at 100 tokens/s; the idle test uses one. The idle result includes the current prototype's **lack of writer reconnect** for continuous PUT/WSS, not an inherent limitation of those protocols.

| Carrier | Baseline completed cases | Buffered upload | Buffered response | Reader cut / from=N | 60 s proxy idle cut | Larger budget |
| --- | ---: | --- | --- | --- | --- | --- |
| `mailbox` | 4/4 | Complete; first 183 ms | Complete; first 184 ms | Complete | Complete | Complete |
| `stream` | 4/4 | Complete; first 60.06 s | Failed (deadline); 59,610/60,000 tokens | Complete | Failed (transport_error); 3,000/6,000 tokens | Complete |
| `post-stream` | 4/4 | Complete; first 183 ms | Failed (deadline); 59,640/60,000 tokens | Complete | Complete | Complete |
| `sse` | 4/4 | Complete; first 186 ms | Failed (deadline); 59,940/60,000 tokens | Complete | Complete | Complete |
| `longpoll` | 4/4 | Complete; first 187 ms | Complete; first 183 ms | Failed (expired_or_acknowledged_gap); 12,000/60,000 tokens | Complete | Complete |
| `h2-stream` | 4/4 | Complete; first 60.06 s | Complete; first 157 ms | Complete | Failed (transport_error); 3,000/6,000 tokens | Complete |
| `wss` | 4/4 | Complete; first 206 ms | Complete; first 203 ms | Complete | Failed (transport_error); 3,000/6,000 tokens | Complete |

## Baseline: ten tunnels at 100 tokens/s each

All rows offer 60,000 synthetic tokens. Latency starts at the scheduled synthetic token time and includes batching and delivery; first-token time also includes connection setup. Percentiles of a failed row describe only the delivered prefix.

| Carrier | First token (slowest tunnel) | p50 | p99 | Relay CPU / client CPU (% of one CPU) | Relay / client peak RAM (MiB) | Relay egress (Mbit/s) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `mailbox` | 183 ms | 96 ms | 130 ms | 1.33% / 2.45% | 12.6 / 8.9 | 2.67 |
| `stream` | 153 ms | 71 ms | 102 ms | 0.98% / 1.56% | 17.2 / 7.1 | 2.28 |
| `post-stream` | 182 ms | 72 ms | 102 ms | 1.33% / 2.51% | 20.0 / 9.0 | 2.61 |
| `sse` | 183 ms | 71 ms | 102 ms | 1.35% / 2.50% | 11.7 / 9.3 | 3.26 |
| `longpoll` | 183 ms | 96 ms | 130 ms | 1.30% / 2.45% | 10.7 / 8.4 | 2.67 |
| `h2-stream` | 155 ms | 71 ms | 102 ms | 1.24% / 1.70% | 16.2 / 7.4 | 2.28 |
| `wss` | 204 ms | 71 ms | 102 ms | 0.75% / 1.48% | 10.5 / 6.4 | 2.27 |

## Receiver reconnection

The receiver intentionally discards the next unverified record after 400 verified records, closes the receive transport and requests `?from=N` for that same record. Cryptographic keys and counters stay in memory. This is not process-crash recovery or a fresh handshake.

| Carrier | Cuts injected | Reader reconnections | Longest reconnect-to-next-data | Outcome |
| --- | ---: | ---: | ---: | --- |
| `mailbox` | 10 | 10 | 152 ms | Complete |
| `stream` | 10 | 10 | 72 ms | Complete |
| `post-stream` | 10 | 10 | 75 ms | Complete |
| `sse` | 10 | 10 | 71 ms | Complete |
| `longpoll` | 10 | 10 | — | Failed (expired_or_acknowledged_gap); 12,000/60,000 tokens |
| `h2-stream` | 10 | 10 | 72 ms | Complete |
| `wss` | 10 | 10 | 173 ms | Complete |

## Interpretation limits

- Completion requires the authenticated End record and all expected tokens. Receiving all tokens without a valid end is still a failure. Completion after a minute of buffering is not live streaming.
- The mailbox is now non-destructive and acknowledges with the next cursor. The destructive `longpoll` remains a reference. The old single-record mailbox is not an eighth variant or the mailbox measured here.
- Heartbeats are deliberately disabled in idle-cut cases. Continuous PUT and WSS writers do not implement outbound replay/reconnect. Reader resume is implemented and measured separately.
- All seven modes use the same AEAD, 30 ms batching and 32-record retained window where applicable. Continuous readers ACK every eight records; mailbox ACKs via its next GET. Queue-full streaming uploads apply backpressure.
- TLS certificates are verified. HTTP/2 negotiation is checked; Nginx terminates frontend HTTP/2 and proxies HTTP/1.1 upstream. No gRPC framework is measured.
- Nginx HTTP body-buffering controls do not apply to an upgraded WebSocket pipe. WSS completing those rows is not evidence that a proxy forbidding Upgrade would allow it. HTTP/2 completed the selected response-buffering case; do not generalize either success or failure to every proxy.
- CPU is cgroup CPU time divided by wall time; 100% means one CPU. Peak memory includes readiness/shaping helpers and page cache. Concurrent lanes use disjoint physical cores but share one host kernel and memory system.
- No application request is replayed. No durable cursor, endpoint restart, complete hostile-relay test, corporate CONNECT/authentication proxy, real WAN or sustained capacity saturation is verified.

## Validation

Workspace formatting, Clippy with warnings denied, the final full workspace test run and the protocol WASM check passed. The experiment's four unit tests and Clippy passed; at that measurement revision the three state counterexamples still reproduced. The subsequent state-only fix replaces them with desired-behavior regression tests; the measured transport data is unchanged. The first workspace test attempt, restricted to spare CPU cores while benchmark networks were active, timed out at the existing five-second WebRTC setup limit in all four multiplex tests. A full unrestricted run after benchmark cleanup passed without changing production code or raising timeouts. That intermittent setup issue is not claimed fixed.
