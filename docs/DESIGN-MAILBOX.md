# Proposed HTTPS delivery contract — awaiting owner review

**Status: contract awaiting approval; S1–S3 state-module work is authorized separately.** Written after the
[seven-carrier decision matrix](../deploy/mailbox-bench/DECISION.md). The owner will review
this contract and the three state tasks below before anyone wires a production HTTPS
transport. Existing WebRTC, `PairingLease`, host/gateway behavior and releases are unchanged.

The isolated benchmark is not a production server, authentication service or persistent
session implementation. Its synthetic bearer value must never be deployed publicly.

## Decision proposed for review

For the requested deployment, use **actual HTTPS request/response traffic**. Recommend the
measured non-destructive mailbox: finite, sequence-addressed PUTs and bounded multi-record
GETs with a `from` cursor. Keep the payload end-to-end encrypted in the existing record
format. No HTTP Upgrade, WebSocket, UDP or TURN is required for this proposed mode.

The seven measured choices and the resulting proposal are:

| Measured carrier | Decision proposed | Reason from the experiment |
| --- | --- | --- |
| Acknowledged multi-record mailbox | **Initial HTTPS candidate** | All four baseline loads and all proxy, receiver-cut, idle and larger-budget cases completed |
| Continuous HTTP/1.1 PUT + GET | Defer | Buffered upload delayed first output by 60 s; buffered response stranded the tail; writer reconnect is missing |
| Finite POST + binary GET stream | Optional later optimization | Lower median delay and successful reader/idle recovery, but buffered response did not complete |
| Finite POST + SSE | Defer unless a browser integration needs it | Similar delay to binary streaming; extra base64 traffic; buffered response did not complete |
| Destructive batched long polling | Reject this delivery rule | Lost an unverified record on receiver disconnect and correctly failed with a gap |
| Continuous HTTP/2 PUT + GET | Defer this upload pattern | Successful read-resume and selected response-buffering test, but upload buffering still delayed output by 60 s; writer reconnect is missing |
| WSS | Outside the ordinary-HTTP requirement | Good throughput and reader recovery; explicit WebSocket upgrade and missing writer reconnect in the prototype |

The recommendation is not "mailboxes are faster". At ten tunnels × 100 synthetic tokens/s,
50 ms RTT, the acknowledged mailbox measured 96 ms median / 130 ms p99 token delay versus
about 71–72 / 102 ms for continuous readers. Its roughly 25 ms median penalty bought the
best tested proxy/failure behavior. These are local, short synthetic runs with resource
limits, not an EPYC/WAN performance guarantee or a production capacity statement.

Only the seven agreed variants were compared. No gRPC framework, additional carrier or
production fallback was implemented. Writer-idle failures are an implementation gap in
this prototype, not proof that HTTP/2 or WebSocket fundamentally cannot reconnect.

## Roles and scope

- **Client:** the local OpenAI-compatible gateway used by a coding agent or other API client.
- **Host:** the endpoint connected to the selected model backend.
- **Relay:** a bounded, volatile HTTPS record store between them. It has no content keys.

Both endpoints initiate outgoing HTTPS requests. This mode is a **permanent relay**, not
signaling followed by a direct peer connection: the relay remains in the data path for the
whole session and handles all ciphertext. It sees network endpoints, opaque room identifiers,
direction, counters, timing and padded size buckets. HTTPS is protocol compliance, not invisibility or a
guarantee that a particular corporate proxy will permit the connection.

A single Rust binary with TLS is sufficient. Nginx is optional. A future static deployment
can use a non-root `scratch` container, a read-only filesystem, dropped capabilities,
bounded RAM and securely supplied read-only credentials. It still uses the host kernel.
No payload storage, access logs or request-body diagnostics are part of this contract.
Host swap/core dumps and any reverse proxy's logging remain separate deployment controls.

## Session and record identity

This proposal defines **delivery after an authenticated session exists**. The chain/state modules now implement S1–S3 as specified below. They are not an approved
or wired production handshake; review and transport-level validation still block production use. Preserve the existing AEAD and direction separation; do not introduce
another cryptographic construction merely to change the carrier.

Each session has an opaque room and two independent directions, `c2h` and `h2c`. The `{session}` segment is the unpadded base64url encoding of the full 32-byte domain-separated
hello/reply transcript hash (43 characters), computed identically by both endpoints. A new
cryptographic session therefore has a different data namespace even when it uses the same room. A transport
reconnection keeps the same live session keys and counters; it is not a new handshake.
Use the room that authenticated the hello (`source_room`), even after the durable current
credential advances. Both sides use fresh CSPRNG inputs for a new hello/accepted attempt.

Data addresses start at 1. In the current encrypted record format the header counter is
zero-based, so address N corresponds to authenticated header counter N−1. The endpoint
checks that relationship, the AEAD tag and strict next counter. The relay can read that
public header for indexing but cannot authenticate or decrypt the contents. A wrong key,
wrong direction, wrong index or invalid tag ends the session; none triggers a permissive
fallback or counter reset.

## Proposed wire delivery operations

The following route spelling is a proposal, not an already implemented production API:

```text
PUT  /v1/box/{room}/{session}/{direction}/{sequence}
GET  /v1/box/{room}/{session}/{direction}?from=N&wait=25
```

Admission credentials belong in authorization headers, never URLs. HTTPS certificate
verification is mandatory. Existing proxy configuration remains explicit. Do not silently
follow redirects to another origin or forward credentials to one.

**PUT:** body is one already sealed record, at most `MAX_PACKET_BYTES`. A 204 means only
that the relay accepted it into volatile memory. It is not proof that the peer received it
or that a model request/tool action executed. Retrying an index uses **identical ciphertext**;
never re-encrypt different bytes under an already used key/counter.

An identical duplicate within the retained record or bounded retry-hash window succeeds
idempotently. Different bytes at the same known index return 409. A retry older than the
retained acknowledgement/hash window returns 410; it must not resurrect a consumed record.

**GET:** `from=N` is the next record the receiver has not yet verified. The request
acknowledges all records below N. Return all currently available **contiguous** records
starting at N, subject to the response count/byte cap; stop at the first gap. A response is
finite and self-delimiting, containing length-prefixed encrypted records. Never skip a
missing counter or return later records as if the gap did not exist.

GET is **non-destructive**. Merely starting, writing or completing an HTTP response is not
an acknowledgement of the records in it. Example:

```text
GET ?from=20  -> records 20, 21, ..., 27
response is lost before verification
GET ?from=20  -> the same retained records can be returned again
receiver verifies 20..27 in order
GET ?from=28  -> acknowledges records below 28 and waits for 28 onward
```

For a multi-record response, the next cursor is **last verified + 1**, not necessarily
request-start + 1. If only 20–23 were verified, resume at 24 and acknowledge only the earlier
prefix. Do not advance a cursor just because bytes arrived in a socket buffer.

A cursor may not acknowledge beyond the range offered by the relay. A request below an
already acknowledged/expired floor returns 410 instead of an empty successful response.
If there is no next record yet, wait up to the bounded `wait` value, then return 204. The
next request carries the same cursor. `wait=0` can acknowledge the final prefix without
holding an unnecessary long poll. The benchmark used a separate final ACK POST; this
`wait=0` spelling is a proposed simplification and still needs production tests.

## Proposed bounds and backpressure

Use the experiment's values as initial reviewable limits, not universal optimal settings:

| Bound | Proposed starting value |
| --- | ---: |
| Plaintext batching timer | At most 30 ms |
| Sealed record size | Existing `MAX_PACKET_BYTES` (12 KiB JSON plus 28 bytes) |
| Records returned by one GET | At most 32 contiguous records |
| Retained, unacknowledged records | At most 32 per room/direction |
| Concurrent finite uploads | At most 8 per direction |
| Message retention | 30 s from acceptance; duplicate PUT does not refresh it |
| Long-poll wait | 0–25 s |
| Global ciphertext budget | 16 MiB |
| Initial active-room budget | 128 |
| Recent acknowledged retry hashes | At most 64 per direction |

The relay removes ciphertext on prefix acknowledgement or expiry. Expiry must run even
when a client disappears; tests used a one-second sweep. Hashes are bounded metadata, not
payload history. Empty-session metadata needs a separate bounded lifetime and must never
recreate an expired session under an old cursor. Propose a 120 s idle lease refreshed by
valid endpoint requests; production validation of that lease remains required.

**One queue backpressure rule:** when the room/direction's 32-record window is full,
hold the PUT until an acknowledgement/expiry frees space, for at most 30 s. Admit only a
bounded number of waiting PUTs and reserve their body memory in the budgets. A PUT that
cannot acquire room capacity within 30 s returns 408 and stores nothing. Never evict an
unexpired record to admit another. Retries use identical bytes and the endpoint's overall
request/session deadline. The reference experiment's short retry loop is not this contract.
429 is reserved for admission/global/per-token resource limits, not a full direction queue.
A global quota rejection must not create a new room or refresh any lifetime.

All shared-use budgets are configurable **globally and per admission token**: live rooms,
live sessions, total retained bytes, pending PUT bytes/count, simultaneous GET waiters,
and request rate/burst. 16 MiB and 128 rooms are private-use starting values, not fixed
buyer limits. Enforce the stricter global/per-token budget before allocating; an operator
must be able to lower them. No unbounded blocked-PUT list is allowed. PUTs holding resources
count against both budgets while blocked. Cancellation/disconnect releases the waiter and
its reserved body memory; it cannot leave an orphaned blocked PUT. Shared public use remains unapproved until tests
at 1,000+ simultaneous tunnels, noisy tenants, expiry and quota exhaustion are available.
The benchmark's public synthetic bearer is not an admission or tenant-isolation design.

## Reconnect and completion contract

A lost HTTP response or TCP/TLS connection may be reopened inside the **same live
cryptographic session**, using `?from=N` from the last verified counter. Retained records
are delivered again until acknowledged. Repeated transport delivery must not result in a
second decrypted/application delivery. Counters must never be reset for a reconnect.

This guarantee is bounded by retention, capacity and the continued existence of both
endpoint processes and relay state. After expiry, relay restart or a missing prefix,
return an explicit failure and end the affected stream. No silent skip, automatic new
session, fabricated End marker or inference/tool retry is allowed.

An endpoint restart is **not** covered by the measured resume behavior. Supporting that
would additionally require a durable cursor/key transaction and a decision about application
side effects. Do not advertise exactly-once model execution or exactly-once tool execution.
The endpoint can claim successful stream completion only after ordered verification through
the authenticated End record. Already emitted output does not turn a truncated stream into
success. Use the existing gateway failure path rather than synthesizing a successful end.

**Peer liveness:** each endpoint sends an encrypted `Frame::Ready` heartbeat after 10 s
without outbound traffic. It uses the normal directional key/counter, retention and queue
limits, and is never forwarded to the application. Any successfully authenticated,
in-sequence record refreshes the peer-liveness clock. HTTP 204, PUT acceptance, unverified
bytes and local sends do not refresh it. After **23 s with no verified peer record**, end
the cryptographic session, discard its live cipher/counter objects and fail in-flight
requests. A TCP/HTTPS reconnection is allowed only inside that live-session window; it
cannot extend it or reset counters. The 30 s relay TTL is not a 30 s promise of recovery.

The 65-second benchmark pause deliberately had no heartbeats or 23 s liveness cutoff.
It tested carrier behavior under proxy idle closure, not this production liveness rule.
With the rule above, such deliberate peer silence ends the session after 23 s; normal idle
peers keep it live with encrypted heartbeats. Long polling remains the default (`wait=25`,
with local cancellation at the liveness deadline); `wait=0` remains supported.

## Handshake on the relay

Handshake slots are room-scoped, separate from session-scoped data:

```text
PUT/GET /v1/hello/{room}/client
PUT/GET /v1/hello/{room}/host/{hello_hash}
```

`hello_hash` is unpadded base64url SHA-256 of the exact sealed hello. The client slot is
replaceable (one latest sealed hello, fixed `HELLO_BYTES`); the host accepts only a fresh,
authentic hello under a current/pending/retired credential it actually holds. A reply slot
is per attempt, fixed `REPLY_BYTES`; identical replacement is idempotent and different bytes
for the same slot are rejected. Reads are non-destructive, wait at most 25 s, and slots have
30 s TTL and per-token/global byte/slot/request limits. Replacing a hello cannot erase
endpoint state or select a stored successor. Replies for different attempts cannot overwrite
each other. Old/incorrect responses must never be interpreted as a new successful session.

The host must rate-limit polling/republication of an unchanged hello; the cache is not a
busy-loop permission. Such an in-memory scheduling cache is optional and never replaces
durable pending state or controls credential validity.

A first-seen hello/reply retains the existing 120 s age / 30 s future tolerance. After the
host durably records an authenticated hello, an exact repeat returns the **stored reply
bytes**, even if that cache entry outlives freshness. It creates no successor, never renews
credential lifetime, and does not restore a cipher. The client still checks reply freshness;
an expired cached reply is not silently refreshed. A cached reply may help a still-live
initiator obtain its successor, but may not start a restarted record session.

The first data record is exactly `[Ready]`. The client must save its successor before it
sends that record. The host must verify it and durably confirm the selected successor
before replying with its own encrypted Ready or releasing any application traffic.
The client also waits for that authenticated host Ready before sending application records,
even though it has already saved its successor.
A known confirmed hello is ignored without a stale/copy warning. New authentic hellos under
one of the three retired generations receive a sealed stale response; still older or
unauthentic hellos are rejected generically. The text is "out of date, possibly copied";
it is not proof of copying or a way to identify the rightful owner of a copied credential.

## Specified state machine for S1–S3

This section specifies the state-module implementation authorized on 29 October 2026.
It does not authorize relay routes, a transport accept loop, configuration or CLI wiring.

**Host durable state:** current credential C; 0–4 pending entries P; newest-first history
of at most three retired generations R; issuance/activity/lifetime metadata. Each pending
entry is `(hello_hash, exact sealed reply, successor)`. All entries belong to C. Each retired
generation contains only its room/hello-role keys plus the hash of the hello that retired
it, so known confirmed replays are suppressed throughout the retained history (including the
last confirmed hello). No record keys, ephemeral DH secret or record counters are persisted.

**Client durable state:** current credential, issuance/activity/lifetime metadata. An
initiator, its DH secret and cipher counters exist only in memory. A restart always uses a
fresh hello under the durably stored credential, never recreates an old cipher at counter 0.

| Event | Required validation | Next host state / result |
| --- | --- | --- |
| Exact hello already in P, same room | Exact hash matches a previously authenticated attempt | Return cached reply only; no new live cipher, no state/lifetime change |
| First-seen hello under C | Authentic, fresh, pending count < 4 | Append a pending entry; save before exposing its reply |
| First-seen hello under C with four entries | Authentic and fresh | Fixed capacity failure; do not evict or replace any successor |
| Fresh hello under a pending successor B | Authentic possession of B proves the client stored it | Retire C with that entry's hello hash; promote B; clear siblings; add this new attempt's successor; atomically save before reply |
| Initial Ready for a live accepted attempt | Matching still-pending transaction and first authenticated record exactly `[Ready]` | Select its successor as C, retire previous C, clear all P, mark paired/refresh activity; atomically save before host Ready/application access |
| Confirmation from a superseded attempt | Pending entry or origin no longer matches | Reject; never roll C back |
| Confirmed hello replay in retained R | Same room and stored confirmed hash | Ignore without warning or mutation |
| New hello under retained R | Authentic/fresh | Sealed stale reply; no mutation or lifetime refresh |
| Missing/invalid/expired/revoked state | No recovery from configuration | Fail closed with fixed Diagnostic text |

Pending entries are not evicted on hello expiry: the client may already hold their
successors. An authenticated holder of any saved successor can recover even when P is full.
Repeated crashes before the client saves any successor can exhaust the four attempts;
recovery then requires an existing saved successor or explicit issuance of a **new pair**.
There is no automatic pruning, bootstrap reset or unbounded history to disguise this bound.
Confirming one branch invalidates its siblings; this remains a one-client-at-a-time pair.

**Cached replies after host restart:** the old Ready cannot be verified because the live
session keys were deliberately discarded. Do not derive/restore them from a cached entry.
The client that saved B retries with a fresh hello under B, obtaining fresh record keys and
a new transcript/session path. This is a new session; old inference requests are not replayed.

**Lifetimes:** explicit issuance creates both endpoint credentials with the same issuance
time and policy. Default unused lifetime is 24 h, paired idle lifetime 30 days; both are
configurable. Optional absolute lifetime is supported but **disabled by default** (90 days
is an example, not a new owner default). Once set, it is measured from issuance and never
renewed. Validity is the minimum of unused/idle deadline and any absolute deadline; expiry
is inclusive at the deadline. Reject wall-clock rollback before the last persisted activity.
Only a committed transition or explicitly verified activity can refresh the idle clock;
hello replays, stale attempts and reads cannot. Unpaired activity cannot extend issuance.
The host is authoritative: a client saving its successor before Ready does not extend the
host's unused deadline. Recovery is always limited by that endpoint's credential validity.

**Issuance versus load:** initialization is explicit, refuses an existing live/revoked/
damaged state, and never runs during load. State version v1 is refused, not guessed or
silently upgraded. Future HTTPS configuration carries **no `pair_key`**: the private state
is the credential. This is a contract for later wiring; the existing WebRTC config is not
changed by this state-only implementation. Do not export a newly issued pair until both
endpoint files are successfully initialized. Partial issuance is not a usable pair.

**Revocation:** stop the running endpoint and its live sessions, acquire its state lock,
write/sync/rename a revoked tombstone, then report success. No keys remain in that tombstone.
Initialization and normal mutations cannot overwrite it. Deleting the state subsequently
still yields missing-state failure, never bootstrap recovery. File revocation cannot erase
cipher objects held by a running process; stopping it is mandatory. This is an API until
CLI/transport wiring is authorized. It cannot prevent an operator deliberately restoring
an old filesystem backup or explicitly reissuing a previously used secret; never do either.

### Crash points and persistence ordering

Every mutation writes a private temporary file, syncs it, renames it, then syncs the containing
directory on Unix. No dependent network action or application release happens before success.
A write failure makes that Store instance unusable; discard live sessions and reopen to
inspect durable state. Never keep processing from a stale in-memory snapshot after a failed
save. Process-exit tests exercise these stages; they cannot simulate physical power loss
or promise directory durability on every filesystem/Windows installation.

| Crash boundary | Durable state on restart | Required recovery |
| --- | --- | --- |
| Before either issuance | Missing | Refuse load; explicit fresh issuance only |
| Between host/client issuance or before export | One or both initialized | Do not export/connect a partial pair; explicit operator recovery |
| Before host pending save | Previous C/P | No reply was released; fresh/repeated hello may be accepted |
| Pending save: before write, after write, after file sync | Previous state (possibly a leftover temp file) | Ignore temp; do not expose the uncommitted reply |
| Pending save: after rename, before/after directory sync | Old or complete new state across power loss; new state after process exit | New state returns cached bytes; old state admits a new attempt; no packet counter restoration |
| After pending save, before/during reply | Pending entry durable | Repeat hello returns exact reply, or a fresh hello adds an entry without replacing others |
| Client receives reply, crashes before successor save | Client still holds previous credential | Fresh hello under it; existing pending successors remain protected |
| Client successor save at each file stage | Old or new client credential | Old uses a fresh hello under old C; new uses a fresh hello under saved successor |
| Client successor saved, before/during Ready | Client successor durable; host pending or confirmed | Fresh hello under successor works in either state |
| Host verifies Ready, confirmation save at each file stage | Pending or confirmed host state | Client already has successor; both states accept its fresh hello; no host Ready/application release on failed save |
| Host confirmation saved, before/during host Ready | Confirmed host; client successor durable | Fresh session on successor; known old hello cannot raise a false stale warning |
| After application traffic starts | Credentials durable; old ciphers/counters lost on process exit | New handshake/session path; old stream fails, no inference/tool retry |
| Activity refresh at each file stage | Old or new bounded deadline | Reload; absolute issuance deadline never changes |
| Revocation at each file stage | Previous live state or complete tombstone | Process stays stopped; retry revocation until durable success; never claim success early |

A copied credential is indistinguishable from its original. Three retired generations bound
the warning history; a still older credential, or a discarded unconfirmed sibling, can only
receive generic rejection. Active stolen state does not expire under idle policy alone;
use explicit revocation or the optional absolute lifetime when that is the desired policy.

## S1–S3 implementation status (29 October 2026)

- **S1 implemented in the state modules:** private bounded `HostChains` transitions;
  four pending hash/reply/successor entries; cached replies without resettable ciphers;
  transcript-derived session IDs; one-shot authenticated initial Ready confirmation;
  atomic promotion and clearing of siblings. The newest three retired entries each retain
  their confirmed-hello hash, extending the requested last-confirmed suppression to all
  three remembered generations without unbounded history.
- **S2 implemented as store APIs:** `initialize`, fail-closed `host`/`client` loads,
  `accept_host`, `commit_client`, `confirm_host`, verified-activity checkpoint and `revoke`.
  New replies are exposed only after the pending save succeeds. Writes poison that Store
  instance on failure. Version-2 credential files record issuance/lifetimes on both sides;
  missing/unsupported/revoked state is never replaced from a code. Absolute lifetime is
  optional and disabled by default. No CLI or HTTPS config reader has been added.
- **S3 implemented with bounded semantics:** three retired generations, sealed stale
  warnings phrased "out of date, possibly copied", confirmed-replay suppression, generic
  rejection beyond that history, and explicit optional absolute expiry/revocation.

The three old counterexamples are now desired-behavior regressions in
[`tests/mailbox_state.rs`](../tests/mailbox_state.rs). State unit tests exercise **45 actual
child-process exits** (nine mutations × five file boundaries), injected I/O failures,
private files/locks, corrupted/unsupported state and lifetime boundaries. Protocol tests
cover pending capacity, losing branches, replay suppression, invalid Ready, session
namespace separation and validated restoration. Process exits do not simulate power loss;
Windows/macOS filesystem durability was not exercised on physical machines here.

No box route, HTTPS transport, host/gateway wiring, legacy pairing lease or WebRTC behavior
was implemented or changed. Review this contract and these state APIs before that work.

## Owner decisions still in force

These were decided before the benchmark and are not changed by it (see revision `866d456`):

1. **Polling:** long poll by default, `wait=0` polling as an option. The contract above keeps both.
2. **Padding:** revised by the owner on 2026-10-30: every sealed record (HTTPS and WebRTC)
   is padded to 1, 2, 4, 8 or 12 KiB of plaintext with trailing JSON whitespace, so older
   receivers decode it unchanged. Counts and timing remain visible; no cover traffic.
3. **`proxy_url`:** in the first version, explicit, never taken from the system.
4. **One pairing, carried along:** the owner pairs once at home and carries the paired client
   state to a customer, for example on a USB stick. One client machine at a time. Whether the
   existing WebRTC mode adopts the same chain is open again, because the owner wants ordinary
   HTTPS for this use case.
5. **Two relays later:** not switched on, but built as if it were: the transport addresses
   relays by index (`sequence % relays`), the configuration rejects a second relay until enabled.
   This contract defines contiguous GETs for one relay; per-relay stride/merge and global
   acknowledgement semantics require specification and tests before enabling a second.
6. **Batching:** at most 30 ms, as in the bounds table.
7. **Lifetimes:** an unused code expires after 24 h, a paired chain after 30 idle days; both are
   configuration values. The S2 lifecycle rules above now specify issuance and verified activity.

## Review findings addressed in this specification

The review at `0d92e20` is resolved in the contract by: the transcript-derived `{session}`
data segment; encrypted 10 s heartbeat / 23 s verified-peer deadline; one blocking-PUT queue
rule with a 30 s bound; explicit room-scoped handshake slots and durable replay behavior;
and configurable per-token/global shared-use limits with 1,000+ tunnel testing still pending.
The S1–S3 implementation and tests follow the state/crash tables above. No relay,
transport or wiring work follows without owner approval of this contract.

## Gate after owner review

S1–S3 state-module implementation is authorized now. Only after the owner approves this
contract and reviews those changes may the box/transport/configuration steps be implemented.
Keep WebRTC and `PairingLease` behavior unchanged unless a separate change is authorized.

Required production tests include every cap and status above; idle clients and expiry;
lost/truncated/duplicated/reordered replies; repeated PUT after ACK; wrong counters/tags;
relay and endpoint crashes; cancellation/backpressure and full-duplex traffic; no secret in
outputs; corporate proxy authentication/buffering; and whole-path gateway/host/backend tests.
The 63 synthetic cases do not replace these tests. No new release or default transport is
approved by the presence of this document.
