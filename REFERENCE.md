# k-lani-link — technical reference

> Network rules, pairing, configuration and limits. Product overview and purchase
> information: [README](README.md). A German translation is kept in
> [docs/REFERENCE.de.md](docs/REFERENCE.de.md); if the two differ, this English
> text is authoritative.

k-lani-link connects AI applications on computer A with a configured backend on
computer B. The locations are up to you. Basalt is the tested reference integration;
for other local OpenAI-compatible servers there is the explicit opt-in
`allow_unverified_backend = true`. The tested Docker template keeps the privacy
check enabled by default.

## Binding network rules

- `relay = "disabled"` is the default. The binary injects no STUN or TURN servers.
  The setup examples explicitly select `stun.marvins.app`, a small STUN service the
  author runs for trying k-lani-link out, without an availability promise; Google alternatives
  are comments until the operator enables them. There is no automatic fallback
  to Google or another data relay.
- ICE/STUN servers and the signaling URL are freely configurable. The STUN service
  learns network addresses. The intermediary sees the public room identifier,
  connection metadata and encrypted packets. SDP, ICE addresses and certificate
  fingerprints sit inside that encryption. Neither service receives AI requests or
  responses.
- Once the tunnel is up, the signaling HTTP client is discarded. There is no
  permanent connection and no background polling of the intermediary during the
  session. Signaling is needed again only for a new pairing with a new code. An
  already opened data channel also aborts a still-pending confirmation from the
  intermediary. Tests watch the actual TCP connections while the replies of both
  signaling backends are deliberately delayed by 30 seconds.
- With TURN disabled, TURN URLs in the configuration and relay candidates in the
  received SDP are rejected. If P2P fails, the error becomes visible; k-lani-link does
  not quietly switch to a server transport.
- TURN works only with `relay = "allowed"` or `"required"` and credentials you
  entered explicitly. The data channel stays encrypted between the devices even
  then. No TURN service is shipped or provided free of charge. Billing and access
  control are the TURN operator's business; a paid TURN service is not part of the
  MVP. The separate purchase link concerns the software license only.
- Local HTTP paths are restricted to loopback. The gateway requires its own API
  key. Basalt credentials stay on the host. Environment proxies and HTTP redirects
  are not used.
- k-lani-link writes no application, connection or error logs and has no telemetry.
  Transport and HTTP logging is compiled out, even in debug builds; `RUST_LOG`
  cannot enable it. Successful starts are silent. Command-line mistakes, an invalid
  configuration, an already used pairing code and a rejected peer produce exactly
  one fixed line on stderr that contains neither keys nor configuration values
  (configuration errors name at most the line number; usage errors name an argument
  but never repeat a supplied value). Connection, transport and I/O errors stay
  silent and are reported as an exit code or HTTP response. `check` reports
  `configuration OK`, and `--help`/`--version` print what was asked for. The
  explicitly invoked credential helper `codex-token` delivers only the API key,
  through a pipe, to Codex; it refuses to write to an interactive terminal.
- Before every upstream request, `/health` must confirm the private inference
  profile. Without it the backend server receives no prompt. This prevents
  accidentally running without privacy mode; a self-declaration does not replace
  inspecting the code. Only `allow_unverified_backend = true` in `host.toml` removes
  this check; `check` and `host` then warn. It is meant for other local servers
  whose logging and history behavior the operator has verified.

## Building

Rust 1.96.0. The repository builds without private neighboring projects.

In the current MVP the native WebRTC adapter lives in k-lani-link's `src/peer.rs`;
the HTTP/streaming protocol lives in `k-lani-link-protocol`. The native MVP is
self-contained; the connector's chat and file-transport interfaces are not used.
A shared transport integration remains a separate step.

```sh
cargo build --release --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo audit
```

The executable is `target/release/k-lani-link` (with `.exe` on Windows). For
the examples below you can use `cargo run -- <command>` instead.

## Creating a device pair

```sh
k-lani-link init --dir pair --signaling-backend ice-exchange \
  --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
```

The command creates a **new** directory containing `host.toml`, `client.toml`,
`pair-code.txt` and `signaling-token.txt`, overwrites no existing files and shows
no keys in the terminal. Random values come from the operating system. On Unix the
directory and files are created with 0700/0600; on Windows the ACLs of the user
profile apply. These files do not belong in a Git repository.

`host.toml` stays on computer B; `client.toml` is moved to computer A over a channel
you already trust. The random `pair_key` has **43 URL-safe Base64 characters** from
32 random bytes. The **first 12 characters determine the room automatically** and
are visible to the intermediary. That leaves 184 secret bits; a short self-chosen
PIN is not supported. The stored `room` field is derived information only and is
recomputed on load.

Alternatively, both devices can be set up independently with the same code:

```sh
k-lani-link new-code --out code.txt
# Move code.txt to the second device over a trusted channel.
# On each device, run with its own new configuration path:
k-lani-link pair --config node.toml --code-file code.txt \
  --signaling-url https://signal.example.invalid --stun stun:stun.marvins.app:3478
```

Without `--code-file` the code is entered without echo in the terminal. A separate
room name is not needed. The local `api_key` for OpenCode/Cline is a different,
longer-lived key than the one-time pairing code.

### Authentication and one-time pairing

Signaling messages are encrypted and authenticated with XChaCha20-Poly1305. Room,
roles, session, time window, SDP and fingerprints are checked. Wrong or tampered
packets end the attempt without automatic retry. Both devices derive separate
session keys per direction with HKDF-SHA256. Every packet on the data channel is
also protected with XChaCha20-Poly1305; sequence numbers prevent replay,
reordering and reflection of packets.

**The code is consumed only after mutual, encrypted key confirmation inside the
opened tunnel.** Failed pairings do not consume it. The session then continues with
its session keys. A new full tunnel requires a new code; the old code is not
reactivated. Whoever holds the secret code can impersonate a peer, so exchanging
the code securely is part of the authentication.

Consumed codes are blocked by empty files with hashed names under
`.link-key-state` next to the configuration, or under `pairing_state_dir`. This
is necessary authentication state, not a chat log; it contains no prompts, responses
or plaintext codes. The directory must be private and kept permanently. After a
process crash a reservation can remain; in that case use a new code. The additional
content encryption has no ephemeral Diffie-Hellman exchange of its own; its
security is not claimed as independent forward secrecy.

### Using an existing ice-exchange service

Replace the signaling placeholder with your existing service. Choose the STUN
providers yourself; these values can be changed freely in **both** configurations:

```toml
signaling_backend = "ice_exchange"
signaling_url = "https://signal.example.invalid"
relay = "disabled"

[[ice_servers]]
urls = [
  "stun:stun.marvins.app:3478",
  # "stun:stun.l.google.com:19302",
  # "stun:stun1.l.google.com:19302",
  # "stun:your-stun-host.example:3478", # Replace before enabling.
]
```

Only uncomment providers you want to contact. Edit or remove the active URL to
use another provider. The [ICE fragment](examples/ice-servers.toml) is also
available separately. The signaling URL and custom STUN hostname are placeholders;
the binary contains no built-in default servers. The generated `signaling_token`
is used only by the optional mailbox backend.

The adapter implements that service's v3 flow: `POST /{room}` to assign roles,
`POST /{room}/offer` with a long poll for the answer, `GET /{room}/offer` and
`POST /{room}/answer`. The existing `sdp` field carries a fixed SDP envelope with
the encrypted pairing packet as its own line; `ice` stays empty. The intermediary
only has to pass this field on unchanged. The real SDP with the gathered ICE
candidates is inside the encrypted packet. Unauthenticated additional ICE arrays
from the intermediary are not accepted. The server needs no change.

The contract is tested against a local reimplementation of these endpoints with
real WebRTC peers. A connection test through an explicitly selected existing
ice-exchange intermediary, with two local peers and synthetic data, also succeeded.
The server was not modified; a test between an office and a home network is still
outstanding.

### Optional local test intermediary

For a fully local trial:

```sh
k-lani-link init --dir local-pair
k-lani-link signal --token-file local-pair/signaling-token.txt
```

The configuration generated this way uses `signaling_backend = "mailbox"` and
`http://127.0.0.1:8788`. The service only relays time-limited SDP messages in
encrypted envelopes and has no inference route. For remote access it must sit
behind HTTPS. This MVP service is designed for private tests, not for a public
multi-customer offering.

If both devices run on the same machine for a local test, their configurations must
use different `pairing_state_dir` directories. In the Docker setup the host volume
and the client directory are already separate.

## Starting host and client

Starting the executable without arguments (or with `ui`) opens the native-client
control panel in a local browser. Its private application directory holds its
configuration, consumed-code markers and an ephemeral panel-address file. The
panel imports a client configuration, starts/stops the same Gateway implementation
as the CLI, reports its health and tests `/v1/models`; it does not change the wire
protocol or enable HTTPS inference. See the [panel guide](deploy/portable/README.md#windows-connect-to-a-remote-model).

Basalt is started separately with the supplied `--privacy` mode and must be
reachable at `http://127.0.0.1:8080` or the configured Unix socket. The Docker
configuration enforces privacy mode, disabled image recognition and the private
Unix socket. The mode suppresses `engine.log`, log buffers, request history,
hardware telemetry and the diagnostics interface; diagnostic and dump flags are
rejected. Details and the update patch: [Basalt integration](integrations/basalt/README.md).
For a different port, change `upstream` in `host.toml`. If Basalt requires an API
key, set `upstream_api_key` on the host only.

```sh
# Backend computer B
k-lani-link check --config pair/host.toml
k-lani-link host --config pair/host.toml

# Client computer A
k-lani-link check --config pair/client.toml
k-lani-link client --config pair/client.toml
```

Client endpoint: `http://127.0.0.1:8787/v1`.
API key: the value of `api_key` in `client.toml`.
Model ID: the actual Basalt model name from `GET /v1/models`.
The additional `GET /health` also requires the API key and reports `connected`,
`recovering`, `pairing_rejected`, `new_code_required` and the configured relay rule.

**Start order.** The order does not matter. The side that starts first waits and
tries again every two seconds until the other one is there; pairing messages are
valid for 120 seconds and a fresh one is created on every attempt. This was checked
with the built-in mailbox using real processes: host first with the client 20
seconds later, and client first with the host 75 seconds later (longer than the
client's 60-second wait for an answer). With an external ice-exchange service its
own waiting windows apply. While waiting, nothing is printed.

### Checking, stopping and further options

`k-lani-link check --config …` validates the file without network access and
reports `configuration OK` or the reason. If the file is accessible to group or
others, a warning appears (`chmod 600`).

`host` and `client` stop in an orderly way on SIGINT **and SIGTERM** (`docker stop`,
systemd): the pairing reservation is released and a private API socket is removed.
After a stop request, running API requests get three seconds to finish; a long
generation does not hold up the shutdown. After SIGKILL or a crash the reservation
can remain; the next start reports it and a new code is needed.

These switches in `host.toml`/`client.toml` are off by default and are not written by
`init`:

- `allow_unverified_backend = true` (host only): do not check the backend's privacy
  profile.
- `include_loopback_candidate = true`: offer only `127.0.0.1` as an ICE candidate, so
  both endpoints must run on the same machine. Intended for tests only.
- `resume_seconds = N` (HTTPS only, set the same value on both endpoints, at most
  `request_timeout_seconds`): after a lost session the host keeps each running response
  for N seconds and the gateway continues it in the next session, without gaps or
  duplicates. The request is never executed twice. While nobody reads, the host stops
  reading from the backend after one response window (64 KiB), so the model is held
  back rather than running ahead; whether a backend then pauses or keeps computing is
  up to the backend. Without a resume within N seconds the backend request is
  cancelled. `0` (default) fails running requests at once. An upload that was still in
  progress is never resumed.

### Several backends through one tunnel

`host.toml` can name up to eight further OpenAI-compatible backends besides `upstream`:

```toml
upstream = "http://127.0.0.1:8080"          # Basalt on the host machine

[[backends]]
upstream = "http://192.168.1.20:1234"       # LM Studio on the Mac mini
allow_unverified_backend = true             # LM Studio has no privacy profile
allow_network_upstream = true               # another machine on the LAN
```

The host asks every backend for `/v1/models` (cached for 30 seconds, refreshed after a
model switch) and sends each chat or responses request to the first backend that lists its
`model`; `upstream` wins when several list the same ID. A model no backend lists goes to
`upstream`, as without this setting. A listed model never falls back to another backend:
if its backend fails the privacy check or is unreachable, the request fails. `GET
/v1/models` returns the merged list of all backends that passed the privacy check, primary
first, so the control panel's connection test shows every model.

A further backend must be on loopback unless `allow_network_upstream = true`; then a
private IP address (RFC 1918, link-local, IPv6 unique-local, or `100.64.0.0/10`, which
Tailscale uses) over `http`, or any host over `https`, is allowed. Hostnames over plain
`http` are refused, because DNS would decide where prompts go. Over plain `http` prompts
cross that network **unencrypted**; `check` warns about it and about unverified backends.
`max_concurrent_requests` is shared by all backends, and model management and its drain
apply to `upstream` only, so a model switch also waits for requests on other backends.

## Supported APIs

The tunnel carries a fixed route table (`ROUTES` in the protocol crate, versioned with
it). Request and response bodies pass through unchanged, so a backend's own API changes
need no change here; a host that lacks a newer row answers 404.

| API | Routes | Typical clients |
| --- | --- | --- |
| OpenAI | `GET /v1/models`, `POST /v1/chat/completions`, `/v1/responses`, `/v1/embeddings` | OpenCode, Cline, Continue, Codex with a custom provider |
| Anthropic Messages | `POST /v1/messages`, `/v1/messages/count_tokens` | Claude Code via `ANTHROPIC_BASE_URL` |

Whether a backend serves a route is up to the backend; check your llama.cpp, LM Studio or
Basalt version. `/v1/embeddings` sends text (for example code for indexing) to the backend
just like chat does.

## Claude Code with your own model

Point the unmodified Claude Code at the local gateway and give it the k-lani-link API key
as its gateway credential:

```sh
export ANTHROPIC_BASE_URL=http://127.0.0.1:8787
export ANTHROPIC_AUTH_TOKEN=<k-lani-link API key>
export ANTHROPIC_MODEL=<model ID served by your backend>
export CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1
export CLAUDE_CODE_MAX_CONTEXT_TOKENS=<context window of your model, e.g. 131072>
```

Set `ANTHROPIC_AUTH_TOKEN` (or `ANTHROPIC_API_KEY`): only then does Claude Code send this
key instead of a saved claude.ai login. The gateway accepts `Authorization: Bearer` and
`x-api-key`, never forwards either, and refuses anything else. k-lani-link never contacts
Anthropic, never uses or relays Claude subscription credentials, and is not a way to use a
Claude plan outside Anthropic's own apps. Requests reach only your own backend.

The gateway accepts Claude Code's `?beta=true` on these routes and drops it; the host
sends `anthropic-version: 2023-06-01` itself and does not forward Claude Code's
`anthropic-beta` list, which local backends do not need. Model discovery
(`GET /v1/models?limit=1000`) is not served, so set the model explicitly. Anthropic's
documentation describes `ANTHROPIC_BASE_URL` gateways but does not support routing Claude
Code to non-Claude models; features that depend on Claude models may not work.

## OpenCode and Cline

**OpenCode 1:** use `examples/opencode.json` as a starting point and replace the
model ID. Create the provider `link` via `/connect` → `Other` and store the
local k-lani-link API key. Then select the model under `/models`.
`@ai-sdk/openai-compatible` uses Chat Completions; `@ai-sdk/openai` is meant for a
client where you explicitly want Responses.
[Official provider documentation](https://opencode.ai/docs/providers/).
The template allows only `link`, disables sharing and requires a deliberate
decision about web and shell tools. Local session histories are permitted. None of
these settings is, by itself, proof that all plugins/telemetry run without outbound
access.

**OpenCode 2:** `examples/opencode-v2.json` uses the V2 schema with `providers`,
provider policies and `reasoning_content`. The isolated
[Docker variant](deploy/opencode/README.md) was tested with 2.2.0, Basalt and real
tool calls. Its starter determines the model ID through k-lani-link.

**Cline:** enter provider `OpenAI Compatible`, base URL `http://127.0.0.1:8787/v1`,
the k-lani-link API key and the Basalt model ID. Set context window and model
capabilities to match the model actually loaded.
[Official configuration](https://docs.cline.bot/provider-config/openai-compatible).
Disable telemetry in Cline and additionally set `telemetry.telemetryLevel = "off"` in
the editor; a separate editor template is in `examples/vscode-privacy-settings.json`.
The Cline code we reviewed distinguishes normal and required telemetry events: the
latter depend on the editor switch, so the Cline switch alone does not block all
events.
[Cline telemetry implementation](https://github.com/cline/cline/blob/775d0398039977ca5447ed39c1a7f178df69aa87/apps/vscode/src/sdk/sdk-telemetry.ts).
Other extensions, MCP servers, updates and deliberately run network tools are not
blanket-controlled by this. An outbound network block for the agent client, with
local k-lani-link access allowed, is the stronger control.

The gateway passes model parameters, tool definitions, tool results and streaming
deltas through unchanged. Which features work is decided by Basalt with the loaded
model. OpenCode 2.2.0 was tested with a real Basalt instance and tool calls; a
Cline agent run of our own has not been verified here.

**Thinking:** `reasoning_content` for Chat Completions and named
`response.reasoning_text.delta` events for Responses are preserved in the running
stream. The tests check that the first reasoning data arrives before the end of the
response. Whether the client shows it expanded depends on its interface. k-lani-link
never invents thinking text.

**Codex:** the connection through the Responses API is tested with CLI 0.147.0 and
desktop core 0.159.0. With the real local Qwen model, the desktop core read files,
edited them with `apply_patch`, verified them and finished the run; thinking was
streamed. Basalt and Codex ran in separate containers without an Internet route, and
the intermediary was switched off after pairing. `codex-profile` creates a provider
profile with a model catalog; `codex-token` delivers the local API key to Codex
exclusively through a pipe. A private Unix listener is available through
`client --api-socket`.
[Setup and reproducible agent test](deploy/codex/README.md).
The desktop interface itself was not switched over; other app connections are not
blocked by a model profile alone.
[OpenAI docs: custom providers](https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway).

## API and limits of the MVP

Basalt remains a separately updatable process. k-lani-link contains no Basalt fork and
no model code. Another OpenAI-compatible inference server can be connected through
`upstream` and its model ID using a verified privacy adapter or the explicit
`allow_unverified_backend = true` opt-in. The latter skips backend privacy
verification and cannot enable our Basalt model controller.
[Generic backend setup and limits](deploy/BACKENDS.md#connect-another-local-server).
New server versions should first pass model listing, streaming, tool
calling and cancellation against this interface before replacing the working
version. The MVP does not download inference-server updates itself.

Supported routes: `GET /v1/models`, `POST /v1/chat/completions` and
`POST /v1/responses`. JSON and SSE responses and HTTP status codes are passed
through. There is no translation between these API formats. Basalt's Responses
implementation is stateless; this is not a complete implementation of all OpenAI
services.

One active request by default; configure `max_concurrent_requests` (1–8) and
`max_queued_requests` (0–16) on both peers for bounded concurrency. Overload returns
HTTP 429. [Model management](deploy/managed/README.md) requires its separate operator key. Maximum
request size is 8 MiB. Responses are transferred in 8 KiB pieces with a bounded send
window. The default total limit is 600 seconds, adjustable with
`request_timeout_seconds` (10–3600). Aborting the local HTTP client stops the
upstream call.

On brief packet loss the stream, session keys and sequence numbers are retained.
After about 8 seconds without peer activity `/health` reports `recovering`; ICE then
has another 15 seconds to restore the same connection. If it returns in time,
pending data is delivered in order, without a new generation or contact with the
intermediary. Buffers stay bounded and live in RAM. The overall request time limit
and the API client's own time limits continue to apply.

After about 23 seconds without peer activity, a closed data channel or any other
final transport error, the running response ends with an error and the upstream
request is aborted. There is no automatic retry of the generation. A new tunnel
needs a new code on both devices. Network or IP changes and process restarts often
require such a rebuild; resuming over a newly negotiated tunnel is not implemented
yet.

The native UDP transport currently uses IPv4. IPv6, convenient pairing,
service installation, signed Windows releases and a browser
extension are later steps. TURN settings are implemented; a real TURN server or
corporate NAT was not used in local testing.

## WASM and browser extensions

```sh
cargo build -p k-lani-link-protocol --target wasm32-unknown-unknown --release
```

The standalone protocol core builds as WASM and exports, through wasm-bindgen,
validation, sealing/opening of the signaling messages and a stateful
`WasmSecureSession` for content encryption and replay protection. This is not yet a
finished browser client. A browser adapter can use `RTCPeerConnection`; the native
process provides the local HTTP server.

A Chrome/Firefox extension could offer pairing, status and a browser chat. For an
HTTP endpoint that OpenCode/Cline call, it still needs a native helper, for example
through [Chrome Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
or [Firefox Native Messaging](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/Native_messaging).
WASM is no guarantee against antivirus or reputation warnings. For native
distribution, reproducible builds and code signing make sense; even signed new
programs may initially trigger SmartScreen warnings.
[Microsoft guidance](https://learn.microsoft.com/windows/apps/package-and-deploy/smartscreen-reputation).

## Data protection

k-lani-link keeps inference traffic between the two chosen devices. The client
computer sends data to the chosen backend computer in the process. Storage
locations, access rights, retention, whether this processing is permitted, and the
settings of Basalt/OpenCode/Cline remain to be clarified. Encryption is a technical
measure, not a GDPR certification.
[European Commission: principles of the GDPR](https://commission.europa.eu/law/law-topic/data-protection/information-business-and-organisations/principles-gdpr_en).
