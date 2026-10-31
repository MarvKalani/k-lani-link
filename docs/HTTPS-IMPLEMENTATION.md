# Implementation checklist against the existing HTTPS decision

Sources, in precedence order:

1. [Seven-carrier decision and measurement limits](../deploy/mailbox-bench/DECISION.md).
2. [Delivery contract, owner decisions and state/crash tables](DESIGN-MAILBOX.md).
3. [Review and historical counterexamples](MAILBOX-REVIEW.md).
4. The existing chain/state APIs and the corrected benchmark retention tests.

The owner requested continuing this HTTPS implementation on 29 October 2026,
then required an explicit check against these documents. This checklist adds no
carrier, cryptographic construction, automatic fallback or public-service approval.
The discarded initial draft was not compiled, committed, pushed or deployed.

## Fixed carrier and boundaries

- Use the acknowledged multi-record mailbox selected in the decision table.
  Finite PUT bodies; finite GET responses; bounded long polling by default.
- Retain ciphertext until the next verified cursor acknowledges its prefix or
  retention expires. Resume at the next unverified record. Never skip a gap.
- Keep the existing AEAD, directional keys, strict counters, transcript-derived
  session namespace and S1–S3 persistence ordering.
- Forward existing HTTP/SSE response bytes through the ordinary gateway/host
  frames. The carrier must not buffer an entire model answer before delivery.
- Leave WebRTC and its pairing lease unchanged. Select HTTPS explicitly.
- Implement one configurable relay. Do not enable a second relay, Cloudflare
  hosting, WSS, continuous uploads, another wire protocol or a new relay epoch
  header as part of this work.

## Implementation and acceptance map

| Contract requirement | Implementation area | Required evidence |
| --- | --- | --- |
| Exact hello/reply routes and session-scoped box routes | Relay routing and volatile store | Path, size, authentication, namespace and reply-conflict tests |
| 32-record contiguous reads and cursor ACK | Retention store | Lost response, partial verified prefix, no destructive read, no cursor beyond offered records |
| Same ciphertext on PUT retry | Sender and retained/hash windows | Duplicate before/after ACK, changed duplicate 409, old retry 410 |
| 30 s retention, 120 s idle metadata lease, bounded sweep | Relay store | Expiry without traffic, no data-request resurrection, no lifetime refresh on duplicate payload |
| Blocking PUT, at most eight uploads per direction | Relay admission and endpoint sender | Full window, predecessor gap, timeout 408, cancellation releases reservations |
| Global and per-token limits | Relay admission | Rooms, sessions, bytes, pending bodies/PUTs, GET waiters, handshake slots, rate/burst, tenant isolation |
| Finite response size and counted in-flight payload memory | Relay response bodies | Slow/cancelled readers cannot escape byte or waiter budgets |
| Existing S1–S3 handshake/state transactions | Endpoint transport | Save before reply, successor before client Ready, host confirmation before host Ready/application traffic |
| Encrypted heartbeat after 10 s, verified-peer deadline 23 s | Established transport | Idle healthy session, silent/dropping relay, no refresh from HTTP success |
| At most 30 ms batching and eight in-flight PUTs | Established transport | Packet/frame caps, ordered receive, full-duplex bounded backpressure |
| TLS verification and explicit proxy only | HTTP client and configuration | Untrusted TLS, redirects, environment proxy ignored, explicit CONNECT/buffering tests |
| Ordinary host/gateway behavior | Tunnel adapter and wiring | Real HTTP/SSE chunks, thinking, tool deltas, parallel requests and cancellation with no UDP path |
| Hostile relay and crash behavior | Transport and integration tests | Drop, truncate, duplicate, reorder, old hello, wrong index/tag, relay loss, endpoint restart, no application retry |
| No logs or payload storage | Runtime/deployment | Fixed diagnostics only, empty operational output, no Docker/proxy logs, read-only runtime, no swap/core dumps |

The baseline limits remain those in the delivery contract: 32 records per GET
and retained direction, 30 s retention/PUT deadline, 0–25 s long poll, 16 MiB
global ciphertext and 128 rooms, with configurable stricter per-token budgets.
Additional implementation bounds must be named and tested, not hidden assumptions.

## Execution order

1. Implement and test relay retention/admission and the specified routes.
2. Add the Tunnel adapter and HTTPS handshake/established transport using the
   existing state modules. Do not restore cipher counters after a restart.
3. Add explicit HTTPS configuration/issuance and host/gateway wiring.
4. Verify the full path with isolated containers, TLS and UDP unavailable; repeat
   the selected mailbox workload/fault cases with recorded resource values.
5. Update usage/status documentation to describe only completed tests. Shared
   public service, release publication and default-transport changes need their
   own scope and acceptance evidence.

Before each push: workspace formatting, Clippy with warnings denied, workspace
tests, protocol WASM check and the product-name guard. No benchmark synthetic
bearer or local pairing state may become a deployed credential.

## Evidence after implementation

The relay, transport, explicit CLI configuration and host/gateway adapter are implemented
as private-test source. [Actual-stack results](../deploy/https/RESULTS.md) record
all four 60-second Docker workloads with UDP unavailable and CPU/RAM for both
containers. The first run **failed the 100-token/s cases**: the host spent one
response credit per small SSE event. The host now coalesces pending upstream items
into the existing 8 KiB chunks while no credit is free; window, frames and contract
are unchanged and the proposed 32-chunk amendment is withdrawn. The corrected run
completes all four workloads (p50 93–125 ms, p99 ≤ 168 ms, no backlog). The relay
also wakes only the affected session direction or handshake room, and endpoints
retry with jittered exponential backoff and bounded `Retry-After`. This preview is
still not production acceptance.

Correctness tests cover relay retention/limits/cancellation, actual TLS, redirect and
certificate refusal, Basic CONNECT, finite-body buffering, partial GET resume, identical
PUT retry, hostile record/reply behavior, actual relay restart, host/gateway SSE and
portable issuance/revocation. Remaining acceptance work is listed with the results,
including full endpoint-process restart wiring, shared-use saturation, native-platform
bundle UX, real corporate WAN and real backend throughput.
