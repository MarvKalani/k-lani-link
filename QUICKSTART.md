# Docker quickstart: connect an AI client to a model on another computer

Before using an employer's or another organisation's network, inform its IT
administrator and obtain the required approval. Internet access alone does not
guarantee a direct connection: NAT, firewalls, proxies and endpoint policies can
block it. Follow the administrator's network and security rules. TURN remains an
explicit, separately configured option; this product does not provide a relay.

This Linux reference setup connects an AI client to Basalt on another computer
over an authenticated, encrypted WebRTC DataChannel. Both machines may be at
any location where their network policies permit the connection. Prompts, reasoning, tool
calls and answer deltas stream through the tunnel. TURN is disabled by default.

For a Windows client with a browser UI and only `D:\OpenCode` mounted, use the
separate [Docker Desktop profile](deploy/desktop/README.md). Its Linux-container
checks do not replace Windows/WSL2 and company-network acceptance testing.

```text
Computer A — AI client                  Computer B — AI backend
OpenCode [network: none]                 Basalt [network: none, GPU]
       │ private Unix socket                   │ private Unix socket
k-lani-link client ═══ encrypted P2P ═══ k-lani-link host
       └──── encrypted setup through your signaling service ────┘
                         disconnected after pairing
```

## Install once, with Internet access

On both computers: Linux x86_64, Docker Engine, Docker Compose v2+ with BuildKit,
Git and Python 3.11+. The reference containers use UID/GID **1000**; adapt users
and volume ownership together if your account differs. Native builds need
Rust 1.96.0; building the supplied container does not need host Rust.

On the model computer: an NVIDIA GPU supported by the pinned Basalt release,
a driver compatible with CUDA 13.2, NVIDIA Container Toolkit, and your own
licensed `.basalt` model file. Model weights are not included or downloaded at
runtime. See [Basalt](https://github.com/jesdga95/basalt) for compatible hardware
and model packs. Our tested pack identifies itself as
**Qwen3.8-Flash-Next IQ3_XXS**, with 262,144 context tokens and int8 KV.

```sh
git clone https://github.com/MarvKalani/k-lani-link.git
cd k-lani-link
docker build -f deploy/link.Dockerfile -t k-lani-link-host:local .
```

Builds and release downloads require Internet. The application containers run
without a network afterward. Docker and the host operating system are trusted
parts of this deployment.

## Default privacy check and other backends

The Basalt Docker profile below keeps privacy verification enabled. An advanced
operator can instead configure a different local OpenAI-compatible server with
`allow_unverified_backend = true` in `host.toml`. This deliberately skips the
backend privacy-profile check and emits a startup warning. It does not prevent
that server from storing requests or contacting other services. Keep the default
for the isolated Basalt setup and review the alternative backend separately.
See [generic backend configuration](deploy/BACKENDS.md#connect-another-local-server)
for LM Studio, llama.cpp, vLLM or Ollama, including route and model-control limits.

## 1. Create a private pair

Run this once on either computer, replacing the signaling placeholder with your
own **existing ice-exchange v3** endpoint. The example explicitly selects
`stun.marvins.app`, a small STUN service the author runs for trying k-lani-link out,
without an availability promise; you can replace it with a STUN service of your choice:

```sh
mkdir -m 700 private
docker run --rm --network none --user 1000:1000 \
  -v "$PWD/private:/setup" k-lani-link-host:local \
  init --dir /setup/pair --signaling-backend ice-exchange \
  --signaling-url https://signal.example.invalid \
  --stun stun:stun.marvins.app:3478
```

The command is deliberately silent. Exit code 0 means success. The newly created
directory contains `host.toml`, `client.toml` and private code files. Do not put
them in Git or a video. Move `host.toml` to the backend computer and `client.toml`
to the client computer through an already trusted channel.

The [ICE configuration example](examples/ice-servers.toml) keeps this STUN active
and lists Google and a custom server as comments. Edit the existing `urls` array
in each TOML file to choose providers; uncommenting an alternative enables contact
with that operator. Repeat `--stun` during creation to select multiple servers.
Without `--stun`, the binary adds no STUN server. There is no automatic fallback.

In **both** TOML files, add the following at the top level, before any
`[[ice_servers]]` section:

```toml
pairing_state_dir = "/var/lib/link"
```

In **host.toml only**, replace the existing `upstream` line and add:

```toml
upstream = "http://localhost"
upstream_socket = "/run/link/basalt.sock"
```

Leave `relay = "disabled"`. The 43-character random code derives the room from
its first 12 characters and authenticates the encrypted signaling messages and
the peer channel. It is consumed only after mutual encrypted confirmation.
Keep pairing-state volumes: **do not use `docker compose down --volumes`** for
normal shutdown. A fully new tunnel requires a fresh code on both nodes.

## 2. Start the model and k-lani-link host on computer B

Copy `deploy/.env.example` to `deploy/.env`. Set absolute paths:

```dotenv
BASALT_MODEL_PATH=/absolute/path/to/your/model.basalt
LINK_HOST_CONFIG=/absolute/path/to/private/host.toml
```

For symlinks, use the model file's resolved path. UID 1000 must be able to read
the private config and model. The pinned build applies the included serving
privacy patch; unpatched Basalt is refused by k-lani-link before it receives a prompt.

```sh
docker compose --env-file deploy/.env -f deploy/compose.yaml build
docker compose --env-file deploy/.env -f deploy/compose.yaml up -d
```

`deploy/basalt.json` configures **262,144 tokens**, int8 KV and vision off.
This is context capacity; a short demo does not prove a fully occupied 256K run.

For the tested 4 GiB RAM / SSD-mapped-expert profile, first ensure host swap is
disabled and adequate RAM remains available, then add
`-f deploy/compose.no-swap.yaml` to the Compose command. This profile sets
`mem_limit = memswap_limit = 4g`, uses `--mmap-experts --prefill 512` and refuses
to start if host swap is enabled. It does not change your system swap settings.
Do not run `swapoff` blindly when RAM is insufficient. Basalt's own RAM guard
can still reject a request under host memory pressure. Fast NVMe reads do not
imply the same speed as resident experts. [Measured hardware](deploy/HARDWARE.md).

## 3. Start the client transport on the other computer

Copy `deploy/client.env.example` to `deploy/client.env` and set
`LINK_CLIENT_CONFIG` to the absolute private `client.toml` path. The backend
host and client must start within the pairing timeout; a delayed peer can
require a restart of an unsuccessful, unconsumed attempt.

```sh
docker compose --env-file deploy/client.env -f deploy/client.compose.yaml up -d client
docker compose --env-file deploy/client.env -f deploy/client.compose.yaml \
  run --rm --no-deps api-test --models
docker compose --env-file deploy/client.env -f deploy/client.compose.yaml \
  run --rm --no-deps api-test
```

With an HTTPS client folder from `init-https` instead of a WebRTC `client.toml`, use
`deploy/client.https.compose.yaml` and set `LINK_CLIENT_DIR` to that folder (it must
belong to UID 1000). The folder is mounted read-write because its pairing state
advances with every session; the API test container sees only `client.toml`.

The last command makes one synthetic request from a separate **network=none**
Python container and displays actual reasoning and answer deltas as received.
Its Docker log driver is disabled. Terminal output is still visible to you;
recording the terminal is your choice. `--prompt` is intended for synthetic
smoke tests, since command-line arguments can be visible to host administrators.

Both k-lani-link transport processes have network access. They use your selected
signaling/STUN services and then direct peer UDP. A direct Internet connection
needs permission to reach the peer, not only the signaling server. With symmetric
NAT or restrictive corporate firewalls, direct ICE can fail. There is no silent
TURN fallback. Explicit TURN opt-in requires `relay = "allowed"` or `"required"`
and your own TURN URLs/credentials in both configurations. You must currently provide your own TURN server and credentials; no relay
service is operated or included by k-lani-link.

## 4. Use OpenCode in another isolated Docker container

On the **client computer**, with host swap disabled:

```sh
docker build -f deploy/opencode/Dockerfile -t k-lani-opencode:2.2.0 .
python3 -B deploy/opencode/prepare.py \
  --client-config /absolute/path/to/private/client.toml \
  --api-volume k-lani-link-client_api_socket
docker compose --env-file .opencode-test/compose.env \
  -f deploy/opencode/compose.yaml build guard
docker compose --env-file .opencode-test/compose.env \
  -f deploy/opencode/compose.yaml up -d --no-build
```

Open **http://127.0.0.1:24244/** in the browser on that computer. OpenCode finds
the remote model through k-lani-link and requires the configured 256K capacity.
It has no network interface except loopback; its model bridge and UI use Unix
sockets. The UI gateway accepts local browser traffic and denies outgoing
IPv4/IPv6/DNS. It is not a public Internet dashboard.

This remote setup uses Compose directly. The `manage.py` resource recorder and
`verify.py` in the same-host lab expect a local Basalt container; they do not
collect remote GPU statistics. Run a numeric sampler on the model computer if
you need those measurements. The full [same-host OpenCode demo](deploy/opencode/README.md)
and [ThinkRail nightly demo](deploy/thinkrail/README.md) remain separately reproducible.
The optional [WordCraft/GridCraft workflow](deploy/opencode/DEMO.md) generates a
synthetic invoice, PDF, spreadsheet and website.

For a native Cline or other OpenAI-compatible client, instead run
`k-lani-link client --config client.toml` and use base URL
`http://127.0.0.1:8787/v1`, the `api_key` from that private file, and the model ID
returned by `/v1/models`. Such a native client is not covered by the Docker
network isolation. A separate browser extension is not included in this MVP.

## Privacy boundary and current limits

The purpose of local AI here is **privacy**: the model and coding agent cannot
open Internet connections in the supplied isolated runtime. The intermediary
receives encrypted setup messages and connection metadata, never prompts or
answers. Its connection is dropped after the authenticated tunnel is ready.
Inference uses DTLS plus authenticated XChaCha20-Poly1305 content encryption.

There are intentional exits: the encrypted peer channel, your local browser,
the agent workspace and files you choose to export. OpenCode keeps local history.
Thus “data can never leave these two containers” and “automatically GDPR compliant”
would be inaccurate. Host administrators and compromised endpoints are outside
this boundary. Approval for processing on the selected backend computer is a separate
matter. The demonstrations use synthetic data.

The default is one active request and no queue. Configure
`max_concurrent_requests` (1–8) and `max_queued_requests` (0–16) on both peers,
together with the backend's capacity, for concurrent requests. Overload returns 429.
The optional [managed Basalt setup](deploy/managed/README.md) adds model loading,
unloading and switching over the existing tunnel with a separate operator key.
Its tested parallel profile uses 6 GiB RAM, no swap and two slots; the earlier
4 GiB single-request recipe above does not establish that capacity. Saved KV
state is not exposed. Conversation history remains the client's responsibility.
See [backend capabilities](deploy/BACKENDS.md) and [security](SECURITY.md).
The two-host recipe is provided for deployment; the recorded measurements were
made with both peers on one host, not a test between independent Internet connections.

[License and EUR 4.99 one-time purchase](LICENSING.md).
