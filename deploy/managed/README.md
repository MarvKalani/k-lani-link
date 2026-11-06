# Model switching and concurrent streams

This opt-in Basalt adapter keeps the authenticated k-lani-link tunnel alive while
replacing the model process. It uses a private Unix control socket inside the
network-isolated backend container. It never mounts the Docker socket, downloads
weights, accepts commands or accepts model paths from the remote API.

## Backend computer

The Qwen examples follow its [published thinking-mode sampling settings](https://huggingface.co/Qwen/Qwen3.8-Flash-Next#best-practices):
temperature 1.0, top-p 0.95, top-k 20, min-p 0, presence penalty 0 and repetition
penalty 1. `sampling` sets validated Basalt launch defaults; request fields still
take precedence. `reasoning_budget_tokens: 0` plus `answer_room: false` removes
both the separate thinking cap and Basalt's automatic answer reservation.
`fit_max_tokens: true` fits requested output to the context remaining after the
actual prompt. The model can finish sooner; using the whole context need not
produce a complete answer. Qwen's template supplies `xhigh` when the client does
not override it. The native 262,144-token context is retained; Qwen recommends
YaRN extension only when longer contexts are needed.

These three optional settings (`sampling`, `answer_room`, `fit_max_tokens`) apply
only to Basalt; older catalogs keep their defaults. Do not copy the zero-budget
meaning to llama.cpp: in that engine zero ends thinking immediately, while an
explicit `reasoning_budget_tokens: -1` uses its unrestricted setting. The example
does not claim independent 262k-thinking/131k-final limits or a tested 1M context.

Build the current images with the production Compose file. Copy
[`models.example.json`](models.example.json) to a private operator directory.
Its IDs (`coder`, `nvfp4`) are the model aliases advertised by Basalt; the files
must already exist. Each profile fixes its context, parallel slots, prefill size
and thinking budget. INT8 KV, privacy mode and vision off are enforced. Basalt
expert memory and prompt batching are operator-controlled as described below.
Only one model is loaded at a time; see below for GGUF models and loading on demand.

Set `LINK_MODEL_CATALOG`, `LINK_CODER_MODEL` and `LINK_NVFP4_MODEL` to absolute local
paths. Keep the ordinary `BASALT_MODEL_PATH` and `LINK_HOST_CONFIG` variables
required by the base deployment. In the host TOML, explicitly enable:

```toml
model_control_socket = "/run/link/control.sock"
max_concurrent_requests = 2
max_queued_requests = 4
```

```sh
docker compose -f deploy/compose.yaml -f deploy/managed/compose.yaml up -d
```

Add `deploy/compose.no-swap.yaml` before the managed override when using the
bounded-memory profile. Set `LINK_BACKEND_MEMORY` to the RAM budget for this
backend, for example `20g` on a workstation with enough free RAM. The same value
sets the memory-plus-swap limit, so the container cannot swap. The compatibility
default is 6 GiB; this can severely limit throughput for a large model.
Host swap must already be disabled (`sudo swapoff -a` on Linux). The supervisor
checks this at startup and before every model load or switch; it does not disable
swap itself. The default example
asks for two slots with 262,144 tokens each. This is a capacity configuration,
not evidence that two full contexts fit a particular GPU. The supervisor checks
the engine's actual model, slot count and context before reporting success;
it refuses a silent fallback to fewer slots. Reduce the profile if it does not fit.

### Basalt performance settings

The example catalog uses `memory_mode: "resident"` and `prefill: "auto"`.
Resident mode keeps CPU experts in RAM when the configured budget permits it;
Basalt applies its memory safety floor and may retain file-backed experts when
the full set does not fit. A large context value alone is not a performance profile.

| Catalog field | Accepted values | When omitted |
| --- | --- | --- |
| `memory_mode` | `"resident"`, `"mmap"` | `"mmap"`, preserving existing catalogs |
| `prefill` | Integer 128–8192; `"auto"`, `"auto:8192"`, `"auto:16384"`, `"auto:32768"` | 512 |
| `prefill_cpu_share` | Finite number from 0 to 1 | Basalt's default |
| `pool_workers` | Integer 1–256 | Basalt's hardware-dependent default |

`memory_mode`, `prefill_cpu_share` and `pool_workers` apply only to Basalt. They
are private operator settings, not remotely supplied engine flags. The control
API still accepts only an allowlisted model ID.

Use [Basalt's probe](https://github.com/jesdga95/basalt/blob/main/docs/probe.md) to
measure the CPU share and worker count on the target machine. Do not copy another
machine's tuning values without measuring. Compare identical prompts and record
the engine version, model, cache reuse, actual output tokens and GPU power limit.
An MTP drafter must exist in the model file; a speculation flag cannot create
one. See [Basalt's model format and conversion kit](https://github.com/jesdga95/basalt/blob/main/docs/model-file.md).

### Custom Basalt expert profiles

Basalt 0.2.1 loads expert placement profiles separately from the model file. Its
bundled profile must match the model's layer and expert counts. For a custom or
pruned pack, an operator can select an existing matching profile in the catalog:

```json
{"id": "coder", "path": "/models/coder.basalt",
 "expert_profile": "/models/profiles/coder.prof"}
```

Mount that file or its directory read-only in a private Compose override. The
supervisor accepts only existing absolute paths, only for the Basalt engine; the
remote model-control API still accepts model IDs only and does not expose paths.
This is a cache-placement ranking, not a replacement for model weights. A neutral
initial ranking is not evidence of a tuned cache or a throughput improvement.
Preserve a model's matching profile before format migration; a 512-expert profile
cannot be used unchanged with a 256-expert pack.

## Client computer

Generate a separate private operator key (a random key file, not a pairing code):

```sh
k-lani-link new-code --out /absolute/private/management-token
```

Set `LINK_MANAGEMENT_TOKEN_FILE` to this file. In the client TOML add:

```toml
management_token_file = "/run/secrets/management-token"
max_concurrent_requests = 2
max_queued_requests = 4
```

```sh
docker compose -f deploy/client.compose.yaml -f deploy/managed/client.compose.yaml up -d client
```

The operator key is mounted only in the transport container. **Do not mount it in
OpenCode, ThinkRail, another agent or its workspace.** The operator credential is checked by the client-side API; enabling the host
control socket authorizes that paired transport peer to use its allowlist. This
is not a separate host-side user identity system. The ordinary inference key
cannot list the operator catalog, load or unload models. Keep its file mode 0600;
using the inference key as the management key is rejected.

```sh
docker compose -f deploy/client.compose.yaml -f deploy/managed/client.compose.yaml exec -T client \
  k-lani-link models --config /run/secrets/client.toml --api-socket /run/link-api/api.sock

docker compose -f deploy/client.compose.yaml -f deploy/managed/client.compose.yaml exec -T client \
  k-lani-link model --config /run/secrets/client.toml --api-socket /run/link-api/api.sock --load nvfp4
```

Use `--load coder` to switch back or `--unload` to release the model. Then select
the loaded alias in the AI application. `/v1/models` lists the currently loaded
model; the administrative catalog includes all operator-approved profiles.
Conversation text remains the client's responsibility. A different model must
process the resent conversation; its KV cache is not transferred or saved.

## Models on demand, including GGUF via llama.cpp

To offer several models on one GPU without anyone switching by hand, add to the host TOML:

```toml
model_control_socket = "/run/link/control.sock"
load_on_demand = true
max_concurrent_requests = 2
max_queued_requests = 8
```

`GET /v1/models` then lists every catalog model with `"loaded": true` or `false`, and a
request naming an unloaded one waits in the queue: the host lets running requests finish,
asks the supervisor to load that model and then sends it and every other waiting request
for it. Requests for the loaded model may still overtake a waiting swap, at most eight or
for 30 seconds, so each model's requests run together instead of one swap per request.
A swap takes as long as the engine needs to load (Basalt packs from fast storage tens of
seconds, a 13 GB GGUF about ten); `request_timeout_seconds` includes that wait. A model
nobody offers gets 404; if a load fails, the requests waiting for that model fail with 503
and nothing is loaded until the next request names a model. Nothing ever falls back to
another model. No operator key is needed for this; explicit `model --load` still works.

The image also contains llama.cpp (pinned by digest in `deploy/basalt.Dockerfile`). A
catalog entry with `"engine": "llama"` names a `.gguf` file, its context, parallel slots
and KV type (`q8_0`, `q4_0` or `f16`; all slots share one KV pool). Mount the folder with
`deploy/managed/gguf.compose.yaml` and `LINK_GGUF_DIR`:

```json
{"id": "Mellum2.1-12B-A2.5B-Thinking-Q8_0", "engine": "llama",
 "path": "/models/gguf/Mellum2.1-12B-A2.5B-Thinking-Q8_0.gguf",
 "context_tokens": 131072, "parallel": 2, "kv": "q4_0", "reasoning_budget_tokens": 8192}
```

llama.cpp has no k-lani-link privacy profile. The supervisor starts it inside the same
network-less container with `--no-webui --no-slots --log-disable --offline`, output
discarded, on the Unix socket only, and reports it as `"privacy": "supervised"`; the
host accepts that report from its own supervisor before every request, and still checks
Basalt's own profile for Basalt models. `allow_unverified_backend` stays off.

## API and scheduling

The separate operator key authorizes `GET /v1/link/models` and
`POST /v1/link/model` with exactly `{"action":"load","model":"coder"}` or
`{"action":"unload"}`. Unknown fields, paths and malformed IDs are refused.
Both routes travel through the same content-encrypted WebRTC channel. Generic
Basalt management routes remain blocked by the privacy patch and tunnel allowlist.

Each inference stream has its own request ID, cancellation token, response window
and timeout. `max_concurrent_requests` is 1..8 (default 1), and
`max_queued_requests` is 0..16 (default 0). These bound admitted requests including
uploads; overload returns 429. Configure both peers consistently and do not set
transport concurrency above the backend's verified capacity. A request body must arrive
within 30 s, both at the local API and at the host; a stalled client gives its slot back
after that (HTTP 408). The overall timeout includes queue time. Each body is capped at
8 MiB. At maximum configuration up to 24 admitted bodies may be resident.

A model operation waits for earlier accepted inference jobs to finish. Later jobs
wait behind it. Cancel a queued operation to discard it; once a model operation
has started it completes even if its caller disconnects. Cancelling an inference
stream affects only that stream. A failed load leaves the backend unloaded and
the tunnel available for recovery. Requests name their model explicitly; they are
never silently sent to the other model or automatically replayed after failure.

Transport cancellation drops the upstream HTTP response before freeing its active
slot. Backend cleanup can finish asynchronously after that disconnect; the engine
must enforce its own execution-slot limit. The managed supervisor waits for the
old model process to exit before starting another model.

Limits to know: all streams of a tunnel share one outgoing queue, so a large upload from
one request briefly delays the flow-control messages of the others while it is sent. If
the engine process exits after a successful load, the supervisor reports no active model
and inference fails until the next `model --load`; it does not restart a crashing engine
on its own.

Installing these new transport binaries requires an initial restart and fresh
pairing for an old consumed session. Subsequent model changes need neither a new
code nor signaling. TURN behavior is unchanged and remains explicit opt-in.

## ThinkRail / OpenCode lab

Create a new lab with `deploy/thinkrail/prepare.py --managed --parallel 2`, the
Coder `--model-file`, `--nvfp4-model-file`, and a new private `--root` directory.
The helper creates the operator key, allowlist and managed profile. The normal
`manage.py up` loads that overlay automatically. Model changes now use:

```sh
python3 -B deploy/thinkrail/manage.py select-model --root /absolute/private/lab --preset nvfp4
```

This operation no longer tears down containers or re-pairs the peers. An old
unmanaged lab must first be migrated explicitly; the command refuses it.
Resource recordings are separate numeric-only opt-ins; payload logging stays off.

## Reproduce the GPU validation

The [28 October 2026 result](validation-20261028.json) records the completed
same-host test: twelve requests in concurrent pairs, both packs on the same RAID 0,
one persistent tunnel and signaling stopped before inference. The successful
6 GiB run had no OOM kill. The 262,144-token capacity per slot was configured,
not filled; a preceding 4 GiB parallel attempt did suffer an OOM kill.

`smoke.py` creates its own disposable backend and transport containers. It requires
both packs on the same filesystem and host swap disabled. It keeps runtime sockets
and numeric snapshots in `/dev/shm`, stores only numerical test results and resource
samples, and removes its containers afterwards. It stops signaling before the first
inference request. Two short requests run concurrently, twice per model, through
Coder → NVFP4 → Coder, followed by an explicit unload. The same consumed-code
markers must remain throughout. Output tokens may include reasoning; an output
cap can end a response before final answer text. This test is not an answer-quality
benchmark, WAN latency test, disk bandwidth test or filled-256K-context test.

```sh
python3 -B deploy/managed/smoke.py \
  --coder /absolute/fast-storage/coder.basalt \
  --nvfp4 /absolute/fast-storage/nvfp4.basalt \
  --output /absolute/private/new-validation-directory --memory 6g --tokens 128
```
