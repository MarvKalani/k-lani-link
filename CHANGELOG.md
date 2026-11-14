# Changes

This file records user-visible changes, including changes retained after the Git
history was consolidated. `Unreleased` describes the current source snapshot;
tagged versions and downloads are listed on
[GitHub Releases](https://github.com/MarvKalani/k-lani-link/releases).

## Unreleased

- The control panel's Roles tab is a playground for role profiles: each role has a
  model, a task and the tools it may use (search the archive, read e-mails, create an
  image or video, hand over to another role). Several roles answer in one
  conversation. No tool is run: a tool call shows what the role asks for, and the user
  gives a simulated result, refuses it or carries out a hand-over. A project has named
  rule sets (for example code style or design rules), and each role uses only the
  rule sets chosen for it. Project, rule sets and roles can be saved to and loaded
  from a JSON file.
- The control-panel chat can dictate through a linked transcription model or the
  browser and read answers with a linked voice or a local device voice; both are
  set in the chat's "Dictation & voice" menu. Models that advertise image or video
  generation get their own Image and Video tabs, each with a form and a result
  list. A generated image can be used as the start frame for a video.
- The control panel's Audio tab reads any text aloud with a Link voice (as a
  downloadable file, long texts in parts) or the device voice, and turns speech
  from the microphone or an audio file of up to 15 minutes into editable text,
  cut at quiet moments into pieces of at most a minute.
- `POST /v1/images/edits` passes through the tunnel as multipart. An image model that
  advertises `max_input_images` gets input images in the panel's Image tab, to edit
  one or combine several; `image_edits = true` assigns the backend. The bundled
  ComfyUI workflow still only creates images from text. Host and client both need
  this version; an older end answers 404 for image edits and model cards.
- `[[model_cards]]` in `host.toml` gives a model a page link, a benchmark link and a
  README. The panel shows them behind a **Model card** button; the host serves the
  README itself under `GET /v1/link/model-cards/{key}`. A missing or oversized README
  stops the host at its start and is reported by `check`. With model cards, a host
  without further backends also answers `GET /v1/models` from its 30-second catalog.
- `k-lani-link ui --client-config` opens the control panel for a running native
  client on its existing OpenAI URL.
- The connected panel opens into a chat workspace with a conversation sidebar;
  model, thinking and dictation sit in the message box. Connection details have
  their own tab.
- The ComfyUI adapter can serve HyperFlow H3 Flash alongside MiniMax H3 Turbo
  when the operator configures the HyperFlow checkpoint and nodes.

## 1.2611.13

- The Basalt image uses v0.3.3 with rebased privacy and numeric-metrics patches.
  Private requests can supply documents inline. File-path and URL reads,
  document caching and completed request-ID retention are disabled in privacy mode.
  A separate workflow checks upstream tags and publishes tested backend images.
  The build and runtime use CUDA 13.5.
- The [local voice adapter](integrations/voice-tts/README.md) exposes Audio8
  and Ming 3.0 with an operator-provided voice reference through the same
  text-to-speech endpoint.
- The [speech Compose profile](deploy/speech/README.md) adds Audio8, Ming and
  Nemotron behind the OpenAI-compatible audio routes. The native client can
  send configured audio routes to local services before the tunnel.
- The browser chat supports dictation and local speech synthesis. Online
  recognition requires explicit confirmation before the microphone starts.
- The ComfyUI adapter serves MiniMax H3 video jobs through the OpenAI-compatible
  `/v1/videos` API. The link carries the multipart first-frame upload and streams
  the resulting MP4. See the [ComfyUI setup](integrations/comfyui/README.md).
- The link carries `POST /v1/images/generations`. The loopback
  [ComfyUI adapter](integrations/comfyui/README.md) exposes the local MiniMax H3
  Image workflow on that route and returns PNGs as base64 JSON.

- k-lani-link can be packed for npm (`npm install -g k-lani-link`) from a published
  release with `scripts/npm-pack.sh <tag>`: a small Node starter plus one
  `@k-lani/link-<os>-<cpu>` package per platform, made from the release binaries
  after checking SHA256SUMS and its signature. Nothing is published automatically.
- The model container no longer runs scripts of ours in Python: its model
  supervisor, single-model entrypoint and health probe are now part of the
  k-lani-link program (`k-lani-link backend supervise|start|health`), copied into
  the image as one static binary. Catalog rules, launch flags, the control socket
  and the group stop are unchanged and covered by Rust tests. Basalt's own server
  remains Python. The host image runs on plain Debian instead of a Python image.
- `deploy/basalt.json` uses Basalt's configuration format 2. The privacy
  profile disables the router's hardware sampler.
- The model image's llama.cpp for GGUF models moves to build b12182 (pinned by
  digest). Mellum 2.1 loads and answers on the GPU with it.
- The control panel frees a request's upload slot once its body is read. Before, a
  request waiting for its answer (for example during a model swap) kept its slot, so
  more than 24 waiting requests across all model computers were refused early.
- The Cloudflare relay counts an IPv4 address written as IPv6 (`::ffff:a.b.c.d`) as
  that IPv4 address for its failure brake, as the Rust relay does, instead of putting
  all of them into one shared /64 counter.
- Pairing and trial clients bound relay responses while receiving them, including
  chunked responses. The control panel bounds concurrent buffered uploads and
  their duration before forwarding, including its chat endpoint.
- The Cloudflare relay groups compressed IPv6 addresses by their actual /64 for
  failed-authentication limits. Its pinned Wrangler toolchain includes the Undici
  security fixes; subnet regressions and the full npm audit run in CI.
- Managed Qwen examples use the published thinking-mode sampling settings and
  native context without a separate thinking cap or automatic answer reservation.
  Basalt fits requested output to the context remaining after the prompt. OpenCode
  starters no longer impose a 16k/64k output cap or force medium reasoning effort.
- The Windows MXC terminal starter is available in `deploy/mxc`, with pinned
  tools, an offline import option and a recorded 12-check sandbox acceptance.
  Its pilot scope and shell-tool limitations are documented in the setup guide.
- The OpenCode Docker starter discovers managed catalogs with several models and
  uses each model's declared context limit. It keeps the already loaded model as
  its initial choice and offers the other models instead of refusing to start.

## 1.2611.4 — 4 November 2026

- Regular release of the current portable application. Historical preview tags and
  their checksums remain unchanged.
- Managed Basalt profiles can choose resident experts, automatic prefill, a measured
  prefill CPU share and the CPU worker count. `LINK_BACKEND_MEMORY` configures the
  container's RAM budget while keeping its swap allowance at zero. Existing catalogs
  retain their prior defaults; the updated examples use resident experts and auto prefill.
- The no-swap guard is checked again before every model load or switch, so enabling
  host swap after supervisor startup cannot bypass the load policy.
- Basalt is pinned to source version 0.2.1, with explicit operator-selected expert
  profiles for custom model geometries and the existing privacy protections.
- The Docker proxy laboratory documents 17 scenarios covering TLS inspection,
  domain policy, trusted CAs, proxy authentication, transport/method fallbacks and
  interruptions. These are reproducible laboratory results, not enterprise approval.
- Updated product presentation and bidirectional relay illustration, with the
  reviewed GIF included in source bundles.

## 1.2611.2-preview.2 — 2 November 2026 (free trial relay, own relay on Cloudflare, status page)

- A relay that refuses every handshake of an access (429 or 503, usually because all the
  connections it allows that access are in use) is no longer an endless "Connecting…":
  after 15 seconds the client reports `relay_busy`, and the control panel shows **Relay
  full** with what helps (end another connection, or a higher limit) and that a new code
  does not. It keeps trying and connects by itself once a connection is free.
- The example relay limits per space (`relay.example.toml`, `ZUGANG.de.md`) now fit one
  person with up to eight model computers: `rooms = 16` instead of 4, more sessions,
  waiting reads and handshake slots. Every connected pairing holds a room and a reconnect
  briefly a second one, so four rooms filled up with two or three computers and a test.
- Relay status page: `/status` (refreshing every 15 s) and `/v1/status` (JSON) show
  totals only: connections, rooms, sessions, requests per minute, memory, uptime and
  trials in use, handed out and refused today. No address, token or content.
- Free trial access: a relay with a `[trial]` section hands out a host and a client
  token of a new space on `POST /v1/trial`, signed with its own secret and checked by
  recomputation, so it stores no trial token. Trials end on a fixed date, are limited
  per address and day and in number, and get small limits of their own. `offer-https`
  without `--admission-token-file` takes such a trial and shows until when it works;
  without `--relay-url` it uses the relay a release was built for.
- Relay on Cloudflare Workers: `deploy/cloudflare-relay` runs the HTTPS mailbox relay in
  a free Cloudflare account, without a server. It keeps only sealed ciphertext, in memory
  only, with the same token rules, limits, pairing by code and failure brake as the PHP
  relay (address rules excepted). `tokens.js` creates the host and client tokens and
  the `TOKENS` secret, which holds only their hashes.

## 1.2611.2-preview.1 — 2 November 2026 (models on demand, DE/EN switch, chat titles)

- Models on demand: with `load_on_demand = true` next to `model_control_socket`, a host
  lists every model its operator allowlisted, loaded or not, and loads the one a request
  names. The swap waits for running requests; requests for the loaded model may still go
  first, at most eight of them or for 30 seconds, so a model's requests run together.
  A request is never answered by another model: one nobody offers gets 404, and when a
  model fails to load, the requests waiting for it fail with 503.
- The managed backend now also runs GGUF models with llama.cpp, inside the same
  network-less container as Basalt (`"engine": "llama"` in the catalog,
  `deploy/managed/gguf.compose.yaml` for the folder). llama.cpp has no privacy profile
  of its own: the supervisor starts it without web UI, slot monitor, log, network or
  downloads and reports so, and the host checks that report before every request.
- Model aliases may contain dots, e.g. `Mellum2.1-12B-Q8_0`, but no path.
- `deploy/mellum.compose.yaml` no longer publishes a TCP port: llama.cpp answered any
  web origin on it, so a web page open on that computer could use the model. The
  container now has no network, runs as uid 1000, answers on a Unix socket in a private
  folder (`LINK_MELLUM_RUN`, `upstream_socket` in the host config) and has its web UI,
  slot monitor and log switched off.
- The control panel has a **DE** / **EN** switch in its header. It changes the language
  at once, open chat windows included, and the panel keeps the choice in its data
  directory; without a choice the panel follows the browser language as before.
- Chat titles can be changed: **✎** in a chat window's title bar, or a double-click on
  the title. A saved chat keeps the new title, and later messages never overwrite it.
  The panel accepts titles of up to 120 characters.
- **Saved chats** is always shown below **New chat** once the panel is connected,
  with a hint while it is empty and how to reopen a chat when it is not.

## 1.2611.1-preview.2 — 1 November 2026 (several model computers, QR pairing, signed updates)

- Several model computers at once: the control panel pairs up to eight hosts and puts
  one local API in front of them, with one base URL and one key. The models of all
  hosts appear in one list; each request goes only to the host that lists its model,
  never to another one when that host is down. A model offered by two hosts appears
  for the second one as `<name>@2`. **+ Add another model computer** pairs one more
  while the others stay connected, **Remove** deletes one pairing. The key an AI app
  already uses keeps working. Each connection now lets eight requests run at once on
  this side; the host's own `max_concurrent_requests` still decides. A QR code pairs
  only once.
- Signed automatic updates: the control panel looks for a newer release on GitHub twice
  a day, downloads it in the background and installs it at the next start, or at once
  with **Update now**. Only releases whose `SHA256SUMS` carry the k-lani-link release
  signature (Ed25519) and are newer than the running version are installed. The check
  sends only the running version number and can be switched off in the panel; hosts
  update on request with `k-lani-link update`. Releases now also contain the bare macOS
  and Linux executables, which the updater downloads. Versions up to 1.2611.1-preview.1
  have no updater and need one more manual download.
- The chat's model list refreshes when it is opened, so a model loaded on the model
  computer later shows up without reconnecting.
- Pairing by QR code, without the relay: `offer-https --qr` shows a QR code in the
  terminal, and **Scan QR code** in the control panel reads it with the camera, or a
  photo or screenshot via **Use a picture instead**. The code carries the client side
  only (relay address, the client's own relay token, the pairing chain's starting secret
  and transport settings); the local API key is made on the client. It needs a separate
  client token, so the host's token never appears on screen, and it stays usable for ten
  minutes by default and only until the first connection replaces the secret.

## 1.2611.1-preview.1 — 1 November 2026 (taskbar icon and chat preview)

- Build with Rust 1.99.1 (was 1.96.0), also in the Docker image and the minimum
  supported version; small code cleanups follow its newer idioms (`let … else`,
  lossless `From` conversions, no redundant clones).
- The browser control panel is a short guided flow: type the relay address and pairing
  code, click **Connect**, and the panel waits, connects and then shows the base URL,
  local API key and model ID with **Copy** buttons. Only the current step is shown;
  disconnect, quit, troubleshooting and `client.toml` import sit under **More options
  and help**. The panel follows the browser language (German or English) and takes
  the k-lani.com look: square edges, hard offset shadows, numbered steps.
- A square icon in the Windows taskbar's notification area and in the macOS menu bar
  shows the connection state by colour; its menu opens the panel, pauses or resumes a
  pairing by code and quits the application. Quitting from the panel removes it too.
  The panel's browser tab shows the same icon in the same colour.
- The control panel has chat windows for trying the model directly: each window is
  its own conversation, can be dragged, resized and closed with ×, and shows the
  streamed answer with its thinking, tokens per second, token count, time to the
  first token and context size. Model and thinking level can change within a chat;
  **Retry** asks again and **Stop** ends an answer early, and a window waits its turn
  while the model computer is busy. A chat is gone when its window closes unless the
  user clicks **Save**; saved chats are private files in the panel's `chats` folder
  and can be reopened or deleted from the panel.
- `offer-https` prints the relay address next to the pairing code, so the operator can
  pass on both at once.
- Pin Basalt to `9175e7a3`: the elastic KV arena hands a deep request's cells back,
  parallel tool results are ordered by call id before the template renders, a
  one-token prompt no longer ends empty, and DirectFile reads files mapped elsewhere
  on Windows. The privacy patch is unchanged; the numeric-metrics patch now keeps
  Basalt's own option and shutdown-order tests green, so all 657 serve tests pass.
  Measured on an RTX 5070 Ti with warm file cache, two cold starts each: decode
  speed is unchanged (82–88 tok/s short requests, 52–57 tok/s at 64k prompt tokens),
  also for a short request right after a 64k one.

## 1.2610.31-preview.1 — 31 October 2026 (pairing by code preview)

- The setup examples describe `stun.marvins.app` as the author's small STUN service
  for trying k-lani-link out, without an availability promise.
- Stopping or disconnecting an HTTPS connection waits until the session has saved its
  last activity checkpoint instead of letting it finish in the background.
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
