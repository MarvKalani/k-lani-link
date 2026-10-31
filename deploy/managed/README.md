# Model switching and concurrent streams

This opt-in Basalt adapter keeps the authenticated k-lani-link tunnel alive while
replacing the model process. It uses a private Unix control socket inside the
network-isolated backend container. It never mounts the Docker socket, downloads
weights, accepts commands or accepts model paths from the remote API.

## Backend computer

Build the current images with the production Compose file. Copy
[`models.example.json`](models.example.json) to a private operator directory.
Its IDs (`coder`, `nvfp4`) are the model aliases advertised by Basalt; the files
must already exist. Each profile fixes its context, parallel slots, prefill size
and thinking budget. INT8 KV, mmap experts, privacy mode and vision off are enforced.
Only one model is loaded at a time.

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
bounded-memory profile. The managed override uses a 6 GiB no-swap container
limit; host swap must already be disabled. The default example
asks for two slots with 262,144 tokens each. This is a capacity configuration,
not evidence that two full contexts fit a particular GPU. The supervisor checks
the engine's actual model, slot count and context before reporting success;
it refuses a silent fallback to fewer slots. Reduce the profile if it does not fit.

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
