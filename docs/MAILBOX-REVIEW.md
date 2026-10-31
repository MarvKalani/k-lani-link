# Mailbox transport review, 28 October 2026

Reviewed `claude/great-pasteur-gq7ca6` at `866d456`. The chain and state-file modules exist;
the production box routes, HTTPS transport and host/gateway wiring do not. These findings
concern that draft, not an already deployed HTTPS feature. WebRTC and `PairingLease` were
not changed during this review.

Production steps 3–5 remain paused until the key-state transaction is corrected and the
owner approves a delivery contract. The owner subsequently required ordinary HTTPS for
this deployment. The [63-case decision matrix](../deploy/mailbox-bench/DECISION.md) now
supports a non-destructive, acknowledged multi-record mailbox as the initial candidate;
see the [proposal awaiting review](DESIGN-MAILBOX.md). Existing WebRTC behavior is unchanged.
The original single-record/destructive-delivery findings below refer to revision `866d456`,
not to the improved experimental mailbox. **Update, 29 October 2026:** S1–S3 are fixed
in the still-unwired state modules; the [specified state machine and crash table](DESIGN-MAILBOX.md)
record the changes and limits. Production box/transport work still awaits owner approval.

## Reproduced defects

The following defects were reproduced at revision `0d92e20` and earlier. The old tests
asserted broken behavior. `python3 deploy/mailbox-bench/reproduce.py` now runs the three
desired-behavior regression tests in `tests/mailbox_state.rs`; it no longer injects an old
counterexample source file. Synthetic credentials and temporary directories only:

1. **P1: replay after a restart can permanently desynchronize the pair.** The host saves
   `cur=A, pending=B`; the client saves B, but its Ready is lost. After the host restarts,
   the in-memory hello replay cache and accepted session are gone. A malicious box can
   replay the still-fresh hello for A. `HostChains::after_accept(Via::Current, C)` overwrites
   B with C. The legitimate client's room B is no longer listened to. See
   [`chain.rs`](../crates/k-lani-link-protocol/src/chain.rs), `after_accept`, lines 449–461
   at the reviewed commit. Persisting just a successor key is insufficient: the pending
   transaction must survive retries and restarts without discarding a successor already
   delivered to the client. An in-memory 150-second hash cache cannot provide that guarantee.
   This demonstrates denial of service/desynchronization, not plaintext recovery.
2. **P1: deleting host state revives the initial credential instead of revoking it.**
   `Store::host` treats a missing `host.chain` as first use, recreates the initial chain
   from the configured code and starts a new expiry clock. A fresh hello using that old
   code succeeds. See [`chain_state.rs`](../src/chain_state.rs), lines 144–156. The draft's
   "delete to revoke" instruction is unsafe with this bootstrap behavior. Initial issuance
   and normal startup must be distinct operations; missing established state must fail
   closed. Deletion also cannot immediately revoke state still held by a running process.
3. **P2: the clone warning covers only one retired generation.** After A→B→C the host
   retains B as `retired`, but no longer listens to A's room. A user with A times out or
   gets an invalid result, not the promised distinct stale warning. See `HostChains::rooms`
   and `confirm`. More fundamentally, copying a credential does not identify its rightful
   owner: the first user of a copy can advance it, and an active stolen credential does
   not expire under an *idle* timeout. The contract needs explicit revocation and a more
   limited description of what the stale-state warning detects.

The existing 20 protocol tests and six state tests passed. Their coverage does not refute
these cases: the existing hello replay test retains the original accepted session and
successor in memory, whereas the counterexample discards them across reopening the store.
State tests inject errors between operations; they do not simulate every power-loss,
filesystem or Windows failure mode. The unused-code clock currently starts at first store
initialization, not necessarily at code issuance; that lifetime also needs clarification.

Validation of this review change: workspace formatting, Clippy with warnings denied,
workspace tests and the protocol's `wasm32-unknown-unknown` check passed. The first complete
test run hit the existing five-second connection-setup timeout at `tests/multiplex.rs:199`
in `an_unread_stream_cannot_block_another_stream`. The targeted rerun and a subsequent full
workspace run passed without modifying WebRTC or raising the timeout. Treat that as an
observed intermittent test/setup issue, not as proof that the first failure was fixed.
At that review revision the standalone benchmark passed formatting and Clippy, and all
three counterexamples reproduced. No claim of finished mailbox security testing follows from those results.

## Delivery and capacity findings in the reviewed revision

**Destructive GET loses the only copy before delivery is acknowledged.** If the server
removes record N and the response connection then fails, the sender may already have seen
PUT success. Retrying GET N cannot recover it. Strict counters correctly prevent accepting
N+1, but the whole stream must then end. Either explicitly promise abort-on-loss or design
bounded acknowledgement/retention and recovery. Idempotence after consumption also needs
definition: with only unread records, a repeated PUT cannot be compared to a deleted record.

**One serial GET per record puts an RTT ceiling on consumption.** Eight simultaneous PUTs
do not remove that ceiling. A 150 ms endpoint-to-relay RTT allows about 6.7 completed GETs/s
on that one receive path even with keep-alive. A 30 ms batching timer can produce over
30 records/s. Backlog then reaches the 32-record cap despite low CPU and bandwidth usage.
The experiment below measures this distinction. Retrying a full queue three times with
an unspecified delay is not a complete backpressure policy.

This is a limitation of the proposed serial-record contract, not of GET or HTTPS in
general. A bounded receive window, multi-record response or different batch policy can
change the result, but each needs a specified ordering, acknowledgement and memory bound.
The comparison stream is not automatically compatible with every corporate proxy either:
some intermediaries buffer request bodies, impose short stream deadlines or reject a
streaming upload. Finite mailbox requests may be useful there. The Nginx test explicitly
disables buffering and is not a substitute for testing the actual permitted proxy path.

The draft's estimate of roughly 20 HTTP requests/s is not a capacity bound. Every record
requires a PUT and GET; reverse credits and heartbeats add traffic. Large prompts fill the
12 KiB record cap independently of the timer. At 1,000 rooms, 32 records × two directions
× 12 KiB is already roughly 750 MiB of possible payload, before sockets, TLS and metadata.
That is a configured worst-case bound, not measured ordinary usage. A shared offering also
needs per-token room/byte limits, connection/waiter/rate limits and global budgets.

## What changes compared with TURN

Both TURN and this HTTPS box relay application traffic throughout the session. The box
cannot disconnect after pairing as a direct WebRTC signaling server can. Content can stay
end-to-end encrypted, while the relay still handles all ciphertext and sees endpoints,
timing and sizes. Its traffic and operating cost do not disappear because it is called
a mailbox.

[TURN supports TCP and TLS between client and server](https://www.rfc-editor.org/rfc/rfc8656).
An HTTPS relay can be useful where permitted infrastructure specifically requires HTTP;
TURN/TLS on port 443 is not the same wire protocol as HTTPS. Neither guarantees permission
or reachability in every corporate network. A
[WebSocket](https://www.rfc-editor.org/rfc/rfc6455) or streaming HTTP connection can carry
the same encrypted records without one round trip per record. QUIC/WebTransport still
depends on UDP and is not a universal solution to UDP restrictions. No TURN implementation
has been benchmarked in this experiment, so there is no measured claim of beating TURN.

**GET-only uploads are not the simpler interoperable contract.** GET bodies have no
generally defined semantics; intermediaries can reject them. Uploading via query strings
puts ciphertext/identifiers in URL handling and logging paths, and GET responses are
cacheable unless explicitly disabled. Encryption protects content but does not eliminate
metadata retention or request replay concerns. Use ordinary POST/PUT request bodies for
upload and a GET stream for download unless a concrete deployment requires something
else. See [RFC 9110, GET](https://www.rfc-editor.org/rfc/rfc9110.html#name-get) and
[sensitive URIs](https://www.rfc-editor.org/rfc/rfc9110.html#name-disclosure-of-sensitive-info).

## Minimal deployment

Nginx is optional. One Rust binary can terminate TLS and relay opaque records. A static
build can run in a `scratch` image as a non-root UID, with a read-only filesystem, dropped
capabilities, no logs, no published administration API and bounded RAM. It still needs
network sockets, entropy, time and securely supplied TLS/admission credentials. The image
contains the executable; "RAM-only messages" does not mean the host has no persistent disk.
[Containers share the host kernel](https://docs.docker.com/get-started/docker-concepts/the-basics/what-is-a-container/).
Host swap, crash dumps, proxy logs and provider telemetry are separate controls.

The dumb relay can be volatile; the endpoints' rotating credentials **must** be durable
for the proposed restart protocol. The requirement of no persistent message storage is
compatible with secret state at the two endpoints, but not with losing that state on
every endpoint restart.

## Experiment

The [isolated benchmark](../deploy/mailbox-bench/README.md) reproduces the data-carrier
choices with real TLS and the existing `SecureSession` AEAD. It does not add a production
transport. [42 completed cases and their limitations](../deploy/mailbox-bench/RESULTS.md)
show the serial-GET queue bottleneck while the continuous stream completes. The workload
ceiling is 1,000 synthetic tokens/s **in total**, including when
spread across 10 or 100 tunnels. This is an architectural experiment, not a public-service
capacity certificate or an EPYC/WAN/LLM throughput measurement.

The later [seven-carrier decision matrix](../deploy/mailbox-bench/DECISION.md) supersedes
that short comparison for carrier selection. It uses 60-second runs at 50/100 tokens/s
with 1/10 tunnels, buffered/idle-cut proxies and receive reconnection via `?from=N`.
The improved mailbox retains records until acknowledgement/expiry and completes the
tested cases. That experiment fixed its delivery rule only. The subsequent S1–S3 state-only
change fixes the credential transitions with regression/crash tests; production wiring
remains unimplemented.
