# Changes

This file records user-visible changes, including changes retained after the Git
history was consolidated. `Unreleased` describes the current source snapshot;
tagged versions and downloads are listed on
[GitHub Releases](https://github.com/MarvKalani/k-lani-link/releases).

## Unreleased

- No changes since 1.2610.31-preview.1.

## 1.2610.31-preview.1 — 31 October 2026 (pairing by code preview)

- The setup examples describe `stun.marvins.app` as the author's small STUN service
  for trying k-lani-link out, without an availability promise.
- Pin Basalt to `5d28effb` (expert fill through the DirectFile pipeline, MTP draft
  context, thinking-loop stop, idle conversations leave VRAM). Both integration patches
  are refreshed; privacy mode also refuses the new on-disk conversation tier. The
  opt-in `--route-prior` / `--route-tail-skip` flags trade answer quality for decode
  speed and stay off.
- Pairing by code: `offer-https` issues the HTTPS pairing and shows an eleven-digit
  code; `join-https` or the control panel ("Enter the pairing code") receives the
  client folder over the relay. SPAKE2 with the code as password; the relay sees only
  public messages and ciphertext; one attempt per code; the host folder is removed
  unless the client confirms. The Rust relay gains short-lived pairing slots, the one
  tokenless route (ten minutes, failure brake on misses). `pair_slots` sets how many
  are open per space and on the whole relay (default 4 and 64, 0 turns pairing by code off).
  The control panel now runs HTTPS pairings, reconnects them at start, shows
  "Access refused by the relay" for a refused token and keeps the client.toml import
  as the alternative. `K_LANI_LINK_DEFAULT_RELAY` presets the relay in a distribution
  build. Both Docker setups pair by code: `deploy/compose.https.yaml` runs the Basalt host
  over the relay with an `offer` service that shows the code, and the Windows desktop
  profile gains `Manage.ps1 -Action Pair` (code on standard input, received into a private
  volume, then imported) and `-Action Start` without a folder once paired. `offer-https`
  and `init-https` take `--upstream` and `--upstream-socket` for the host folder;
  `join-https` reads a piped code and `LINK_PROXY_URL`. The PHP relay offers the same pairing slots, limits and failure brake. The HTTP/3 client tries every resolved relay
  address at once.
- Optional relay access rules (Rust and PHP relay): tokens naming the same `space` share
  rooms and limits, so host and client hold separate tokens and each can be revoked
  alone; per token `expires` (last valid day), `allow_ips` (addresses/networks) and
  `max_ips_per_day` (distinct addresses within 24 h, IPv6 per /64, salted hashes only);
  a failure brake (`failed_auth_per_minute`) answers `429` to guessing addresses but
  never to a valid token. Address rules fail closed without a peer address and are
  refused with `behind_tls_proxy`. `init-https --client-admission-token-file` issues
  the client folder with its own token. The WordPress page creates a host+client token
  pair and edits the rules. Relay configuration errors now name the line and reason.
  Guide: `deploy/https/ZUGANG.de.md`.
- Self-tuning for small web hosts: the PHP relay announces `X-K-Lani-Relay: wait=…;
  writes=…` and lets only `max_waiters` reads wait at once (others answer at once).
  Endpoints honor the announced limits, halve write parallelism on overload and grow it
  back slowly, and pause up to 2 s between reads while empty answers come back early.
  Lab (8 PHP workers): 4 tunnels now deliver everything (p50 0.6–0.8 s; before: 40 %),
  10 tunnels deliver everything with growing delay instead of failing.
- PHP relay review fixes: expiry no longer keeps metadata of deleted records (an expired
  record was acknowledged as present); expired ciphertext is swept by requests, a
  per-minute WP-Cron job and `standalone/sweep.php` for system cron; deactivation closes
  the new plugin fast path (`relay.php`, no WordPress start per request); fast path and
  REST route share one storage folder derived from the site salt. The source bundle
  admits the reviewed PHP relay files and refuses a real `config.php` or stored data.
- Review fixes in the transport: a hello whose first PUT answer was lost now counts
  toward the unanswered-hello guard; finished responses kept for resume are bounded (16);
  HTTP/3 uploads reach the router as a stream, so token and quota checks come before
  any body is read.
- PHP relay for ordinary web hosting (`deploy/php-relay`): a WordPress plugin with access
  tokens and a plain-PHP variant, same mailbox semantics, stores only ciphertext
  (expiring after 30 s, deleted at the next request or cron run). Endpoints accept a relay URL with a path and step past web firewalls by
  themselves: PUT, then POST, then the record wrapped in JSON (OWASP CRS blocks PUT and
  binary bodies with 403). The Rust relay accepts POST and JSON-wrapped writes too.
  Lab with WordPress behind ModSecurity CRS: 1–2 tunnels smooth, 4 slow, 10 fail with
  8 PHP workers. German setup guide: `deploy/php-relay/ANLEITUNG.de.md`.
- Experimental HTTP/3 (QUIC) for the HTTPS mailbox: endpoints opt in with
  `[https] http3 = true`, the relay with `http3 = true` (UDP twin of `listen`). The first
  QUIC failure moves the same session to TCP (HTTP/2 or HTTP/1.1) without a new
  handshake; `http3` with `proxy_url` is refused. Lab: no gain without packet loss and
  about 1.7× relay CPU; with 1 % loss about 25 % lower p99 and much faster setup.
- HTTPS mailbox over HTTP/2: endpoints and relay negotiate HTTP/2 by ALPN and fall back
  to HTTP/1.1 when the relay or a TLS-inspecting proxy does not offer it; the record
  protocol is unchanged. `http1_only = true` (endpoint `[https]` or relay) forces
  HTTP/1.1. The relay bounds HTTP/2 to 64 streams per connection and 16 KiB headers.
  Lab: same latency, first token about 200 ms sooner, one connection instead of several,
  slightly more CPU (relay 3.5 % vs 2.5 % of one CPU at 10 tunnels).
- The isolated OpenCode desktop profile can use the HTTPS mailbox: the first start
  imports the client folder's pairing into a private volume (later starts keep it,
  another pair is refused) and `Manage.ps1 -ProxyUrl` sets a customer's proxy. The
  preparation container now reads the client's folder instead of one file. Linux:
  `deploy/client.https.compose.yaml`. `deploy/desktop/https_smoke.py` tests the
  transport end to end with a real relay and host.
- Carry the Anthropic Messages API (`/v1/messages`, `/v1/messages/count_tokens`) for an
  unmodified Claude Code pointed at your own backend, and OpenAI `/v1/embeddings`. Routes
  come from one versioned table in the protocol crate; an older host answers 404. The
  gateway accepts `x-api-key` as well as Bearer, accepts only Claude Code's `?beta=true`
  on Anthropic routes and answers there with Anthropic-shaped errors; the host pins
  `anthropic-version` and does not forward `anthropic-beta`. No wire format change.
- Host: up to eight further OpenAI-compatible `[[backends]]` behind one tunnel. Requests
  go to the backend whose `/v1/models` lists their model (unlisted models to `upstream`,
  never a silent fallback for listed ones); `/v1/models` returns the merged list of
  privacy-checked backends. Backends on other machines need `allow_network_upstream`
  and a private IP over http or any host over https; `check` warns about plain HTTP.
- Pad every encrypted record (HTTPS and WebRTC) to 1, 2, 4, 8 or 12 KiB of plaintext,
  so record sizes no longer reveal exact token, prompt or tool-call lengths. The padding
  is trailing JSON whitespace inside the encryption: no wire version change, and older
  receivers decode padded records unchanged. Record counts and timing stay visible.
- Optional `resume_seconds` (HTTPS, both endpoints, default 0): a response running when
  the session is lost survives that long and continues in the next session without gaps
  or duplicates; the request is never executed twice and an unresumed backend request is
  cancelled at the deadline. Adds the `resume` frame, sent only when enabled. A host
  with resume 0 answers a resume with an error; an older host treats the unknown frame
  as a protocol error and stops, so enable it only after updating both endpoints.
- HTTPS gateway: after a lost session the client negotiates a fresh session by itself
  (new handshake and keys, jittered backoff). In-flight requests fail and the host drops
  their backend requests; nothing is replayed. `/health` reports `reconnecting`. Retries
  stop with `reconnect_required` after three handshakes whose reply was lost after
  delivery, so they cannot use up the host's four pending attempts. Transient relay
  errors at startup are retried instead of being reported as a rejected pairing.
- `include_loopback_candidate = true` works again and now binds only `127.0.0.1`.
  webrtc 0.21 expands `0.0.0.0` to every non-loopback interface, so the flag had no
  effect: local tests connected over every Docker/VM bridge (about 20 here, 400
  candidate pairs per peer), dropped STUN packets under parallel load and intermittently
  missed the five-second setup deadline in `tests/multiplex.rs`. No deadline changed.
- Fix the HTTPS application-path throughput limit without a contract or wire change:
  the host now coalesces small upstream response items into the existing 8 KiB
  credited chunks instead of spending one of eight credits per SSE event. All four
  50/100-token/s × 1/10-tunnel Docker workloads now complete (p50 93–125 ms). The
  proposed 32-chunk HTTPS-only window is withdrawn. WebRTC uses the same path.
- HTTPS relay: wake only waiters of the affected session direction or handshake room
  and expire only the addressed room on data requests, instead of waking every waiter
  and sweeping all rooms per request. Admission refusals carry `Retry-After: 1`.
- HTTPS endpoints retry with jittered exponential backoff (100 ms to 2 s) and honor a
  relay `Retry-After` up to 5 s instead of retrying every 100 ms.
- Implement the explicitly selected acknowledged multi-record HTTPS mailbox as an
  opt-in source preview, preserving WebRTC and its one-time pairing lease. Add
  bounded volatile relay storage, global/per-token quotas, native TLS, strict
  session namespaces, cursor acknowledgement, identical ciphertext retries and
  pipelined finite PUTs. Use the existing S1–S3 durable handshake ordering, encrypted
  10-second idle heartbeats and a 23-second verified-peer deadline. Never start a
  fresh client session or replay application work automatically after interruption.
- Add `init-https`, `https-relay` and `revoke-https`, portable current-state folders,
  explicit proxy configuration, host/gateway wiring and a static scratch relay image.
  Test TLS, Basic CONNECT, buffering, thinking/tool SSE, parallel requests,
  cancellation, hostile records, reply replay, response loss and cursor resume.
  Add an isolated actual-stack Docker workload runner with enforced UDP refusal.
  See `deploy/https/README.md` for measured limits and remaining acceptance work.
  The native panel, Docker Desktop launcher and published preview.3 binaries do
  not support HTTPS state bundles; no public relay, release or default is enabled.

- Select `stun.marvins.app` explicitly in remote setup examples and add an editable
  ICE fragment with Google/custom alternatives commented out. Server choice stays
  with the operator; no runtime default, automatic fallback or TURN is enabled.

- Align the Docker environment example with the verified Basalt source pin
  `71116cc3`, matching the Compose defaults and included privacy/metrics patches.

## 1.2610.29-preview.3 — 29 October 2026 (connection panel preview)

- Starting the executable without arguments opens a local browser control panel;
  Windows double-click launches it without a persistent empty console. Existing
  CLI commands remain available. Reopening returns to the running instance.
- Import and validate a private client configuration, connect/disconnect, inspect
  live tunnel state and test backend model discovery through the encrypted tunnel.
  Show the local inference API key only on request. Store no conversation logs.
- Protect the panel with loopback binding, a private random token, Host/Origin
  checks, CSP and bounded requests/responses. Keep one-time-code state across
  imports and restarts. No tray icon, Docker control or HTTPS inference is enabled.
- Add native lifecycle/authentication tests and a real encrypted same-host
  backend-test round trip that still works after signaling stops.

- Put direct Windows x64/ARM64 downloads and the current Docker setup ZIP at the
  top of both READMEs. Refresh executable links on release events, including
  published previews, only after all Windows packages and checksums are uploaded.
  Distinguish published binaries from the newer source on main.

- Use HTTP/1.1 and bounded network retries for Cargo downloads in the check
  workflow. Pin cargo-audit 0.22.2 and select the repository's Rust 1.96.0
  toolchain explicitly when installing it. Audit failures still fail CI.

- Explicitly unlock the still-unwired pairing-state store when its owner drops.
  A concurrently spawned process could briefly retain the file description and
  prevent reopening the state. Add a deterministic duplicate-handle regression;
  no pairing protocol, WebRTC path or delivery contract changes.

- Add an opt-in Docker Desktop OpenCode browser profile and Windows PowerShell
  launcher. Mount only `D:\OpenCode` into the network-disabled agent; separate
  pairing keys from its inference credential, keep its home/history on tmpfs,
  and retain consumed-code state in a private volume. Reuse the outbound-blocked
  browser gateway and keep TURN disabled. Windows/WSL2 and real corporate-network
  acceptance remain pending; no production HTTPS transport is enabled.

- Restore a public English business-workflow video and FilmCraft GIF previews
  after reviewing the historical footage for the name change. Label the archive
  edit, operator assistance and measurement limits. Update both READMEs with the
  demo and current experimental HTTPS status; preserve existing binary downloads.

- Fix the still-unwired pairing-chain state APIs: retain four pending successors,
  replay saved replies without restoring cipher counters, retain three retired
  generations, and confirm only a still-pending initial Ready. Separate explicit
  issuance from fail-closed loads; add durable revocation and bounded lifetimes.
  State files use version 2 and reject the old format. Add the three regression
  tests and 45 child-process crash cases. Specify the reviewed HTTPS contract;
  no box, transport, CLI/configuration wiring or WebRTC behavior changes.

- Compare exactly seven HTTPS/WS experiment carriers in 63 cases with 60-second
  50/100-token/s loads, 1/10 tunnels, proxy buffering and idle cuts, and receive
  reconnection. Add non-destructive multi-record mailbox reads and acknowledged
  retention to the experiment. Record CPU/RAM for both test containers and a
  decision table. Propose an HTTPS delivery contract and three unresolved state
  tasks for owner review; no production transport, default or pairing change.
- Add a review of the draft HTTPS mailbox, three reproducible chain-state
  counterexamples, and an isolated Docker carrier benchmark with 42 measured cases.
  Native Rust TLS and optional Nginx are compared under explicit CPU, RAM, latency
  and bandwidth limits. No production HTTPS transport or WebRTC fallback is enabled.
- Describe backend support by the supported OpenAI-compatible chat endpoints,
  rather than a fixed engine list. Add Strata and Maya as examples in both READMEs
  and distinguish generic connectivity, tested privacy profiles, model management
  and the Open Responses specification. No transport behavior changes.

## 1.2610.29 — 28 October 2026 (portable preview)

### Added

- Portable native build matrix for Windows x64/ARM64, macOS Intel/Apple Silicon
  and Linux x64/ARM64. Installer-free executables, versioned packages, SHA-256
  checksums, build metadata and Rust dependency notices. Initial binaries are
  unsigned previews; no WASM runtime is added.
- Native platform tests include encrypted loopback WebRTC streaming and bounded
  concurrent requests. Unix-only host administration remains explicitly guarded;
  a Windows client can control a managed Linux backend over its existing tunnel.
- Configurable 1–8 concurrent streams and a 0–16 request FIFO queue, with
  independent cancellation and response flow control. Defaults stay one stream
  and no queue; both peers and the backend must be configured consistently.
- Opt-in allowlisted Basalt model supervisor with a private Unix control socket,
  separate client-side operator credential and CLI controls. Model operations wait
  for earlier requests and hold later requests until complete. Model reloads
  preserve the existing authenticated tunnel without new signaling or pairing.
- Operator routes `GET /v1/link/models` and `POST /v1/link/model`; ordinary
  inference credentials cannot invoke them. No arbitrary model paths or commands
  are accepted. Only one model pack is loaded at a time; KV state is not saved.
- Generic local-backend opt-in, `allow_unverified_backend = true`, with a startup
  warning. It skips backend privacy verification, not tunnel authentication or
  encryption. LM Studio, llama.cpp, vLLM and Ollama connection instructions now
  distinguish this transport option from unimplemented, unvalidated dedicated
  deployment and model-management adapters.
- German and English purchase-terms and withdrawal drafts, clearly marked for
  review. They are not activated in checkout and do not alter existing purchases.

### Changed

- Rename the product, executable, Rust packages, image tags and documentation to
  k-lani-link. Reopen the source repository with one snapshot commit, preserving
  the prior Git history privately. License sales stay paused. Old recordings stay
  private; current artwork is WebP and website editorial changes are disclosed.
- Complete the identifier rename, including protocol domains and backend privacy
  fields. Upgrade both peers together and generate fresh pairing codes; earlier
  builds are incompatible and their codes must never be reused.
- Pin Basalt to `71116cc3`, refresh privacy/metrics patches and check its actual
  model, parallel slots and context before accepting a managed model load.
- Use 6 GiB backend RAM without swap for the managed parallel profile. The
  earlier 4 GiB single-request profile is not a validated parallel configuration.
- Numeric resource reporting follows the loaded model. Decode throughput for
  parallel inference is labeled as the total across active slots.
- Documentation is English first. Update English/German README and technical
  references, quickstart, model-control instructions and backend capability limits.
- Exclude historical demo GIFs from source bundles and remove maintainer-only
  Stripe notes, optional k-lani-bot tooling/MCP bridge and video scripts.

Licensed Work is now 1.2610.29; the Change Date stays 28 October 2030, within four years of first
distribution. Portable package status and signing limitations are documented in
the [portable guide](deploy/portable/README.md).

### Fixed

- Keep cancelled forwarding jobs in the host scheduler until their upstream
  response is dropped. Model-barrier tests now exercise normal stream completion
  explicitly instead of depending on HTTP-server disconnect timing.
- Keep the operator-key diagnostic regression valid on Windows as well as Unix.
- Identifiers that still carried the former product name are renamed: environment
  variables (`LINK_*`), the backend health field (`link_privacy`), the API error type,
  container paths, the container user and service names, provider IDs in the examples,
  the pairing-state directory, lab script options and the protocol labels (DataChannel
  label, key-derivation and one-time-code domain strings, SDP carrier). Builds from before
  this change cannot pair with it, and earlier pairing codes are void. See
  `docs/CODER-HANDOVER.md` for the mapping and `docs/REBRANDING.md` for the consequences.
- `deploy/check-names.py` runs in CI and fails when the former name reappears.
- A request body must now arrive within 30 s at the local API as well (the host already
  enforced this). Before, a stalled client kept its capacity slot for the whole request
  timeout.
- The operator key file is checked with one fixed reason per problem (missing, not
  readable by this user, link or directory, too large, open to group or others, wrong
  length or characters, same as the API key) instead of one generic sentence.
- Fix: SIGTERM (`docker stop`, systemd) now stops host and client in an orderly way.
  Before, the pairing reservation stayed behind and the next start failed silently
  until a new code was created; the API socket also stayed on disk.
- Command-line mistakes, invalid configuration, a used pairing code and a rejected
  peer now print one fixed, secret-free line to stderr; `check` prints
  `configuration OK`. Connection, transport and I/O errors remain silent.
- Diagnostics never echo raw command-line values or unknown TOML field names,
  which may contain pasted secrets. Regression tests cover both cases.
- The message for a rejected pairing no longer blames only the code or the clocks: it
  also covers an unusable rendezvous answer (wrong URL, proxy or captive-portal page).
- Usage errors name the affected argument (and the allowed values of an enum) but still
  never repeat a supplied value. A bare invocation shows the usage text again.
- A stop request gives running API requests three seconds instead of waiting for a whole
  generation, so `docker stop` finishes cleanly and removes the API socket.
- Warn when a configuration file is accessible by group or others.
- The loopback ICE candidate is now opt-in (`include_loopback_candidate`).
- `init` creates the pair directory with mode 0700 atomically.
- Suppress the newer Basalt answer-budget output in privacy mode.

### Validation

- CI: `cargo audit` and a release build run on every push.
- Rust tests cover independent stream cancellation, queue bounds and model-control
  ordering. Python tests cover the model supervisor and resource reporting.
- Real GPU validation: Coder → NVFP4 → Coder → unload on one authenticated tunnel,
  with signaling stopped and both packs on the same RAID 0 filesystem. Twelve
  requests completed in concurrent pairs. The 6 GiB run had no OOM kill; an earlier
  4 GiB parallel attempt did. Each slot was configured for 262,144 tokens, but
  prompts were short and output capped at 128 tokens, including reasoning.
  This does not establish full-context capacity, answer quality or WAN throughput.
  [Numeric results](deploy/managed/validation-20261028.json)
  · [Reproduction instructions](deploy/managed/README.md#reproduce-the-gpu-validation).

## 1.2610.28 — 28 October 2026

First public MVP, source release.

- OpenAI-compatible chat and Responses streaming over authenticated WebRTC,
  preserving reasoning, tool calls and cancellation.
- Additional XChaCha20-Poly1305 content encryption; one-time pairing code,
  encrypted signaling, disconnect from intermediary after tunnel establishment.
- TURN disabled by default, explicit ICE servers, no fallback relay.
- Basalt privacy patch and network=none Docker profile; isolated OpenCode 2.2.0
  and ThinkRail nightly examples, optional local document tools.
- Standalone remote-client Compose profile and English two-computer setup.
- Optional numeric-only token timing export and hardware/resource dashboard.
- Saturated-queue close regression and restricted inference-route tests.
- BSL 1.1, free production use below the stated USD 100,000 threshold, and
  EUR 4.99 one-time user licenses covering all 1.x releases.

Known limits: one active request per tunnel, no model administration or saved
KV API, no automatic tunnel re-pairing, no ready-made browser extension,
no independent security audit or general GDPR certification. See README and
QUICKSTART for the deployment and validation boundaries.
