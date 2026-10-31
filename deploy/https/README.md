# Explicit HTTPS mailbox preview

This implements the **acknowledged multi-record mailbox** selected in the
[seven-carrier decision](../mailbox-bench/DECISION.md), using the existing
[delivery and state contract](../../docs/DESIGN-MAILBOX.md).
It is an opt-in source preview for private testing. Published preview.3 executables,
the native connection panel and the Docker Desktop launcher do **not** support these
paired-state folders yet. No automatic WebRTC fallback or public shared service is enabled.
The first full-path run exposed a response flow-control bottleneck at 100 tokens/s;
after the [documented correction](RESULTS.md) all four 50/100-token/s × 1/10-tunnel
workloads complete with about 0.1 s median token delay. That is a synthetic lab result,
not a release: real WAN/proxies and real inference over HTTPS remain unverified.

Both peers use ordinary finite HTTPS PUTs and GETs to one operator-selected relay.
The relay carries **all tunnel ciphertext**, so its bandwidth and hosting cost differ
from the direct WebRTC path. No STUN, UDP or TURN is used in this mode. TLS is verified;
an additional authenticated end-to-end session protects content from the relay.
The relay still sees IP addresses, room/session identifiers, directions, record counts and
timing. Record sizes show only a padding bucket: every sealed record is padded inside the
encryption to 1, 2, 4, 8 or 12 KiB, so the length of a token, prompt or tool call is not
visible to the byte. One record per streamed event still reveals the event rhythm. HTTPS is not a guarantee that a corporate firewall will allow it;
obtain the network administrator's permission.

## Build from source

The ordinary native build provides the new CLI commands:

```sh
cargo build --release --locked
```

For the minimal Linux x64 Docker image, install a musl C compiler (for example
`musl-tools` on Debian/Ubuntu), then build a static binary:

```sh
rustup target add x86_64-unknown-linux-musl
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl --bin k-lani-link --jobs 2
docker build -f deploy/https/Dockerfile -t k-lani-link-https:local .
```

The final `scratch` image contains the executable and license documents. It needs the
host Linux kernel; it has no shell, Nginx, init system or package manager. A certificate,
private key and admission token are mounted read-only. The relay has no payload files
or persistent mailbox volume. Disable host swap if ciphertext must never be paged to
disk; `memswap_limit` alone cannot prevent host-wide swap or host backups/dumps.

## Configure a private relay

Create a dedicated directory outside the repository. Copy `relay.example.toml` into it
as `relay.toml`; supply a valid TLS server chain as `server.crt` and its key as
`server.key`. Certificates must cover the hostname/IP used by both peers. Use your own
certificate provisioning; no ACME client or certificate updater runs inside this image.

Create random, private admission tokens without printing them, one per device:

```sh
target/release/k-lani-link new-code --out /private/relay/host-token
target/release/k-lani-link new-code --out /private/relay/client-token
```

The example container runs as UID/GID 1000. Make the directory accessible only to that
operator (`0700`), the token/key private (`0600`), and readable by UID 1000. Adapt the
Compose user and ownership together if needed. Do not loosen key permissions to fix a
mount error. Admission tokens are **not** content keys. Tokens that name the same
`space` share rooms and limits, so host and client of a pair may hold separate tokens
(the example does this); a token without `space` has an isolated namespace of its own,
and both peers of a pair must then use that same token.

```sh
export LINK_HTTPS_RELAY_DIR=/private/relay
docker compose -f deploy/https/compose.yaml up -d --build
```

Compose publishes TCP 8789 on loopback by default. For a remote private deployment,
set `LINK_HTTPS_BIND` to an explicitly permitted bind address, such as `0.0.0.0:443`,
and configure the firewall accordingly. This is **not** an HTTP-only listener: the
example terminates TLS natively. Keep the default Docker seccomp policy and capability
drop. No UDP port is published unless you enable HTTP/3 (below). Endpoint HTTPS mode
never initializes the WebRTC stack.

**Experimental HTTP/3 (QUIC).** With `http3 = true` in `relay.toml` the relay also serves
the same mailbox over QUIC on the UDP twin of `listen`; uncomment the `/udp` port line in
`compose.yaml` and allow that UDP port in the server firewall (for example UDP 443 next to
TCP 443). It needs `tls_cert`/`tls_key` in the relay itself. Endpoints opt in with
`http3 = true` in their `[https]` table. They try QUIC first and switch to TCP (HTTP/2 or
HTTP/1.1) for five minutes at the first QUIC failure, inside the same session: no new
handshake, keys and cursors unchanged. A network that silently drops UDP costs at most a
1.5-second attempt. `http3` cannot be combined with `proxy_url`, because QUIC cannot pass
an HTTP proxy. The relay applies the same per-token quotas, its own connection cap,
address validation and no 0-RTT. The `h3` crate is a 0.0.x release; treat this path as
experimental.

An existing trusted TLS proxy is optional: omit `tls_cert`/`tls_key`, explicitly set
`behind_tls_proxy = true`, and keep the cleartext listener on a private, restricted
path. The proxy must preserve methods, authorization, query strings and finite binary
bodies, allow GET waits of at least 25 seconds and PUT waits of at least 30 seconds,
and disable access/error/body logging, caching and disk spill. Do not expose this
cleartext listener publicly. A CDN or provider may retain metadata outside our control.

The relay offers HTTP/2 by ALPN and falls back to HTTP/1.1 per connection; endpoints do
the same. HTTP/2 carries all of an endpoint's PUTs and waiting GETs on one connection
(at most 64 streams); every request still passes the per-token quotas below. Set
`http1_only = true` in `relay.toml` or in an endpoint's `[https]` table to force
HTTP/1.1, for example to rule out a proxy problem.

Global and per-space quotas are configurable in `relay.toml`. The example is a starting
budget for a single user, not a tested public-service capacity promise. Limits include
rooms, sessions, retained/in-flight bytes, waiting uploads/GETs, handshake slots and
request rate/burst. The TLS listener has a separate connection cap.

### Restricting access (optional)

Every option is off unless set; all are checked in memory, before any body is read.
German step-by-step guide: [ZUGANG.de.md](ZUGANG.de.md).

| Setting | Where | Effect |
| --- | --- | --- |
| `space = "name"` | `[[tokens]]` | Tokens with the same name share rooms and limits: one token per device, each revocable alone (remove its entry, restart). Limits are set on the space's first token; later tokens without `[tokens.limits]` take them. |
| `expires = "YYYY-MM-DD"` | `[[tokens]]` | Last valid day (UTC); `401` from the following midnight. |
| `allow_ips = [...]` | `[[tokens]]` | Only these addresses or networks (`203.0.113.0/24`, `2001:db8::/48`). |
| `max_ips_per_day = N` | `[[tokens]]` | At most N distinct addresses (IPv6 per /64) within 24 hours; known addresses keep working, new ones get `401`. Only salted hashes are kept, in memory. |
| `failed_auth_per_minute = N` | top level | An address with more refused requests per minute gets `429` for `failed_auth_ban_minutes` (default 10). Valid tokens are never held back. |

Address rules and the brake need the relay to see client addresses, so they are refused
together with `behind_tls_proxy`; without a known address, a token with address rules is
refused. Refusals use `401` like a wrong token, never `403` (endpoints read `403` as a
firewall rule). On `401` an endpoint does not retry: the host process exits with "HTTPS
relay rejected access or protocol" and stays down until restarted (a service manager
with automatic restart does that). Set `max_ips_per_day` generously: a dual-stack
machine counts once for IPv4 and once for its IPv6 /64, and a home connection may get a
new address every day. A mobile client changes addresses often: give it a higher
`max_ips_per_day` (12) rather than `allow_ips`. Configuration errors name the line and
the reason.

The single-user limits in the example carried 1, 2 and 4 synthetic tunnels at 100
tokens/s each without loss (`https-probe`, one machine, no WAN; p50 22 ms, p99 34 ms,
slowest first token 0.12 s), so they restrict misuse without slowing one person down.

## Issue and transfer paired endpoint state

Run issuance **once**, in a new private directory on a trusted computer:

```sh
target/release/k-lani-link init-https \
  --dir /private/https-pair \
  --relay-url https://relay.example.com \
  --admission-token-file /private/relay/host-token \
  --client-admission-token-file /private/relay/client-token
```

`--client-admission-token-file` gives the client folder its own token; both tokens must
name the same `space` in `relay.toml`. Without it, both folders get the one token.

For a private CA, add `--ca-cert-file /private/ca.pem`. Never disable TLS verification.
Use `--proxy-url http://proxy.example:3128` if an explicitly permitted proxy is required.
Proxy credentials, if configured in that URL, become secret configuration data.
System proxy variables are deliberately ignored. The tested proxy authentication is
Basic CONNECT; NTLM/Kerberos/PAC and TLS interception are not verified.

Both durable endpoint states are saved **before** either portable configuration is
exported. The result contains `host/host.toml` and `client/client.toml`, each with its
own `state/`, relay token and optional CA. Paths are relative to the configuration
folder and stay valid when that entire folder moves. No bootstrap code is exported in
the TOML. Defaults: unused issuance expires after 24 hours; paired state after 30 idle
days. `--unused-seconds`, `--idle-seconds` and `--absolute-seconds` set explicit limits.

Keep `host/` beside the inference server. Transfer only the complete **client folder**
through a trusted private channel to the other computer, then remove obsolete copies.
Use one client instance at a time. State rotates; copying the original folder repeatedly
is not a reconnect strategy. A copied valid credential cannot be distinguished from
its owner. Do not commit these folders, mount them into an AI agent or store them in
the project directory being edited.

Set `upstream` or `upstream_socket` in `host.toml` to the existing model service. The
normal privacy-profile check remains enabled. Other OpenAI-compatible backends require
the existing explicit `allow_unverified_backend = true` opt-in and an independently
verified isolation/logging configuration. The transport adds no model Internet access.

```sh
# Beside the inference server:
target/release/k-lani-link host --config /private/https-pair/host/host.toml
# On the client computer, using its transferred, CURRENT folder:
k-lani-link client --config /private/client/client.toml
```

Configure the AI application to use `http://127.0.0.1:8787/v1` and the local `api_key`
in `client.toml`. That API key permits inference; it is different from both rotating
pairing state and relay admission. The existing host/gateway still carry HTTP/SSE
chunks, thinking fields, tool deltas, parallel requests and cancellation. Concurrency
must be configured consistently on both endpoints and supported by the backend.
For isolated applications, use the existing Unix-socket API and network-disabled app
container recipe; keep state accessible only to the transport container.

## Pair by code instead of copying the client folder

The host side issues the pairing and shows an eleven-digit code; the other computer types
it and receives exactly what `init-https` would have written into `client/`. Nothing has
to be copied, mailed or carried on a stick.

```sh
# Beside the inference server:
target/release/k-lani-link offer-https \
  --dir /private/https-pair \
  --relay-url https://relay.example.com \
  --admission-token-file /private/relay/host-token \
  --client-admission-token-file /private/relay/client-token
# Shows e.g. "Pairing code: 472-1234-5678" and waits up to ten minutes.

# On the client computer (or in the control panel: "Enter the pairing code"):
k-lani-link join-https --dir /private/client --relay-url https://relay.example.com
```

- The first three digits name a short-lived slot on the relay, the other eight are the
  password of a SPAKE2 exchange. The relay sees only the public SPAKE2 messages and
  ciphertext; it never learns the code or the pairing.
- One attempt per code: the client answers once, and the host closes the slot when that
  answer does not prove the code. A wrong code therefore costs the code, never the
  pairing. Guessing has a chance of 1 in 10^8 per code and needs the code's slot number.
- The client proves the code before the host sends anything secret. The host's state
  exists before the bundle leaves; the pairing counts only after the client confirms its
  durable write. Without that confirmation within ten minutes the host folder is removed.
- `join-https` brings its own `--ca-cert-file` and `--proxy-url`; the bundle carries
  neither. The control panel has no proxy field yet: behind a mandatory explicit proxy,
  pair with `join-https --proxy-url`. Both sides may spell the relay address
  differently (case, default port, trailing slash); it is normalized before use. A distribution can preset the relay with `K_LANI_LINK_DEFAULT_RELAY` at build
  time; public builds have none and ask for it.
- **The one exception to "no token, no answer":** the client side of an open slot
  (`GET /v1/pair/{slot}/a`, `POST /v1/pair/{slot}/b`, `GET /v1/pair/{slot}/c`,
  `POST /v1/pair/{slot}/d`) needs no token. Only a token holder can open a slot
  (`POST /v1/pair`), read the answers or close it. Slots last ten minutes, bodies of at
  most 64 B / 128 B / 8 KiB / 64 B, memory only. `pair_slots` bounds how many are open:
  under `[global]` for the whole relay (at most 900), in a token's `[tokens.limits]` for
  its space; the defaults are 64 for the relay and 4 per space. `pair_slots = 0` turns
  pairing by code off there, and `offer-https` then says so.
  Tokenless reads never wait, and requests without a token draw on their own budget of
  20 per second, never on the tunnels' budgets. Unknown, expired and used slots answer `404` and count
  against the failure brake; every other route still answers `401` without a token.
- The PHP relay offers the same slots with the same limits (`pair_slots` in its
  configuration). Behind a web application firewall that refuses binary request bodies,
  pairing by code may fail; use `init-https` and copy the client folder there.

## Interruption, restart and revocation

A lost HTTP response is retried within the **same live cipher session**. GET resumes at
the next unverified record; PUT resends identical sealed bytes. It never repeats an
inference request or tool call. Encrypted idle heartbeats run after 10 seconds; no
authenticated peer record for 23 seconds ends the session. HTTP success alone does
not prove that the peer is alive. Retained records expire after 30 seconds.

A missing/expired record or restarted relay ends affected streams. In-flight requests
fail, and the host drops their backend requests, so generation stops instead of running
unread. The client gateway then negotiates a **fresh session** by itself (new handshake,
new keys, jittered backoff between failed attempts) and the host keeps accepting client
handshakes. A request is never executed twice. Any application-level decision to
resubmit work is the operator's or the coding tool's. `/health` reports `reconnecting`
while this runs.

Optional `resume_seconds = N` on both endpoints changes the first step: the host keeps
each running response for N seconds instead of cancelling it, and the gateway holds the
caller's response open and continues it in the next session from the last Data frame
it received. Only the unconfirmed part of the same response (at most one 64 KiB window
plus the terminal frame) is sent again; the backend is held back by the missing credits
meanwhile. Without a resume within N seconds the backend request is cancelled. Uploads
still in progress fail. Default `0` keeps the behavior above.

Each new hello that the host accepts but whose reply is lost occupies one of the host's
four pending attempts until a later handshake completes. The gateway counts an attempt
only when its hello reached the relay and the transport then failed before a reply came;
an absent host does not count. After three such attempts in a row it stops and reports
`reconnect_required`, which leaves the last attempt for an explicit client restart. A
lost answer to the hello itself counts too, for example a proxy that answers 502 while
the relay is down: then the gateway stops after about three attempts (some six minutes),
and an explicit restart continues. An
endpoint process restart also creates a new session; it never restores old encryption
counters or an old stream cursor.

Stop a role before revoking its local state:

```sh
k-lani-link revoke-https --role host --config /private/host/host.toml
```

Revoking the host stops that pair's future access at the host. Revoking a local client
copy does not remotely erase another copy. Missing, damaged, expired, revoked or locked
state fails closed; startup never reissues it. A new pair needs an explicit new directory.
Do not restore a bootstrap code or overwrite rotating state from an old backup.

## Reproduce the bounded Docker measurements

Build the lab-only probe separately:

```sh
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
  cargo build --release --locked --target x86_64-unknown-linux-musl --example https-probe --jobs 2
python3 -B deploy/https/run-lab.py --seconds 60 --profile small
```

Requirements: Linux x64, Docker/cgroup v2, UID 1000, Python 3, OpenSSL, swap disabled,
a trusted default-deny seccomp profile (default `/usr/share/containers/seccomp.json`),
and the instrumentation image built by the earlier `deploy/mailbox-bench/run.py`.
`--seccomp-base` and `--tools-image` select those inputs explicitly. The runner never
changes production services, host swap or host network shaping. It creates an internal
Docker network without published ports. Disposable helpers with NET_ADMIN shape only
the test network namespaces; actual relay/endpoint processes have no capabilities.
The laboratory seccomp profile narrows the trusted allowlist to stream sockets and
verifies UDP socket refusal before traffic. Production Compose keeps Docker's own policy.

The four fixed workloads are 50/100 synthetic 161-byte SSE tokens/s at 1/10 tunnels,
60 seconds each. Small relay budget: 2 vCPU / 1536 MiB; `--profile large`: 4 vCPU /
7168 MiB. Both use 50 ms shaped endpoint-to-relay RTT and 500 Mbit/s egress. These
budgets reference the earlier small-VPS comparison; the physical CPU remains the local Ryzen.
The load container runs the **actual host, gateway and HTTPS transport**, with both
endpoint roles in one container; a separate scratch container runs the actual relay.
No LLM runs. This measures one selected implementation, not another carrier variant.

Only aggregate latency/completion, cgroup CPU/peak RAM, image/source hashes and machine
metadata are retained under `.release/https-lab-*`. CPU is percent of one CPU over the
measurement interval; RAM is the cgroup high-water mark. The synthetic model and both
endpoint roles share the load container, so its statistics are not standalone agent or
GPU consumption. No payload/state/certificate key is included in public results. The
TLS/fault/CONNECT/buffering and S1–S3 process-crash tests are separate correctness evidence.

Real corporate WAN, Windows/macOS private-state UX, NTLM/Kerberos, real Basalt throughput
over this new carrier and shared-use saturation at 1000+ tunnels remain unverified.
