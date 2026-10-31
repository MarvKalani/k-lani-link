# Backend capabilities and model management

Reviewed on 28 October 2026. k-lani-link 1.2610.29 forwards inference; it does not
pretend that all OpenAI-compatible servers share model-management semantics.

The product connects computer X to computer Y, independent of their location.
**The transport can connect any local backend that implements the supported
OpenAI-compatible chat endpoints.** Basalt is the first qualified deployment.
Other engines need their own integration and privacy validation; naming an engine
as an API example does not mark it as tested. The current source accepts a generic
local server with the explicit `allow_unverified_backend = true` host setting;
this skips the privacy-profile admission check, warns at startup and is not a
claim that the backend is privacy-qualified. The secure default remains false. A host-side gateway to an external provider such as
OpenAI is also planned; it needs an explicit cloud policy because that provider
receives the inference data. Dedicated deployment/privacy profiles and management
adapters for other engines are not shipped in 1.2610.29.

| Operation | Current k-lani-link MVP | Backend capability |
| --- | --- | --- |
| List the loaded model | `GET /v1/models` | Basalt supplies model ID and context metadata |
| Stream chat, reasoning, tool calls | Passes SSE through unchanged | Depends on model and client support |
| Responses API | Passes through; no API translation | Pinned Basalt implementation is stateless |
| Load/unload/switch models over the tunnel | Opt-in private Basalt supervisor; tunnel stays connected | Only operator-allowlisted model IDs |
| Select Coder/NVFP4 in the lab | `select-model` uses the managed profile | No peer restart or new pairing code |
| Concurrent inference | Configurable 1–8 streams with a bounded queue; default 1 | The backend's capacity and transport limits must be configured together |
| Save/restore conversation history | Client responsibility | OpenCode/ThinkRail may keep local history |
| Save/restore engine KV state | Not exposed | Engine/model/format dependent; not portable between models |

## API conventions and Open Responses

"OpenAI-compatible" is a widely used API compatibility convention, especially
for [Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create).
It is not a promise that every backend implements every OpenAI endpoint, model
feature or extension. Chat Completions uses JSON requests and, for streaming,
[server-sent events (SSE)](https://developers.openai.com/api/docs/guides/streaming-responses).

[Open Responses](https://www.openresponses.org/specification) is an open,
provider-independent specification based on the Responses API, covering shared
request/response schemas, streaming events and tool interactions. It gives
implementers a more explicit interoperability contract. It is distinct from the
Chat Completions format and does not define a common local-engine loading or
KV-cache management interface.

k-lani-link forwards the HTTP/SSE subset on its allowed routes. It does not
translate Chat Completions into Responses, implement the optional Responses
WebSocket transport or expose `/v1/responses/compact`. Forwarding `/v1/responses`
alone is not evidence of full Open Responses conformance; the complete
client/tunnel/backend combination still needs compatibility testing. Reasoning
and tool-call details depend on the selected model, server and client.

## Connect another local server

The following engines document OpenAI-compatible endpoints:
[Strata](https://github.com/niko1221/strata),
[Maya](https://github.com/mw00/project-maya),
[LM Studio](https://lmstudio.ai/docs/developer/openai-compat),
[llama.cpp](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md),
[vLLM](https://docs.vllm.ai/en/latest/serving/online_serving/openai_compatible_server/)
and [Ollama](https://docs.ollama.com/api/openai-compatibility).
Their API documentation establishes a connection path, not a completed
k-lani-link integration test or privacy assessment.

For example, with LM Studio listening locally on port 1234 on the backend
computer, change these top-level fields in the host's private configuration:

```toml
upstream = "http://127.0.0.1:1234"
allow_unverified_backend = true
```

Use the actual local port of your chosen server. `upstream` is the origin only,
without `/v1`. Remove an old Basalt `upstream_socket` when changing to TCP, and
omit `model_control_socket`: our model supervisor requires the verified privacy
profile and cannot be combined with this opt-out. Set `upstream_api_key` privately
if that local server requires authentication. Do not publish the host config.

Only `GET /v1/models`, `POST /v1/chat/completions`, `POST /v1/responses`,
`POST /v1/embeddings`, `POST /v1/messages` and `POST /v1/messages/count_tokens` are
forwarded for inference. Ollama's native `/api` routes, engine administration, audio
and other services are outside that allowlist. The backend must
implement the route the client uses; k-lani-link does not convert Chat Completions
to Responses or normalize reasoning/tool-call formats. SSE and request JSON,
including the `model` field, are passed through. If an engine itself selects
models from that field, it can do so without replacing the tunnel; this does
not provide a universal load/unload API.

The generic setting skips only the privacy-profile admission check. Peer
authentication, content encryption, route restrictions and explicit TURN opt-in
remain in effect. It does not turn off backend logs, history, telemetry, downloads
or cloud features. Our network-isolated Docker recipe still needs a compatible
private Unix-socket connection for each alternative engine. A local TCP example
alone is not equivalent to the tested `network=none` Basalt deployment.

## Basalt and Maya

Pinned Basalt exposes `--parallel` (1–8), backed by native `--batch`. By default,
the total `--max-context` is shared across slots: 262,144 / 4 gives 65,536 tokens
per slot. `--slot-full-context` gives every slot the full capacity, at greater
memory cost. Its normal server has load/unload and status routes, but our privacy
patch disables them. k-lani-link's route allowlist also rejects management calls.
Sources: [pinned serving code](https://github.com/jesdga95/basalt/tree/5d28effbc0dfe9f8a554cc895e6de8858cbc4bb0/serve)
and [configuration](https://github.com/jesdga95/basalt/blob/5d28effbc0dfe9f8a554cc895e6de8858cbc4bb0/docs/configuration/index.md).

[Maya v1.1.34](https://github.com/mw00/project-maya/releases/tag/v1.1.34) adds
the CYBER-FROST Qwen3.8 path. Its `--parallel` support applies to GLM, DeepSeek
and MiniMax; it must not be inferred for the new Qwen path. At the reviewed
commit, Qwen's settings allowlist has no `parallel` key. MiniMax parallel slots
retain their KV state in VRAM and disable some memory-saving paths. Sources:
[model settings](https://github.com/mw00/project-maya/blob/249f9a87d1a9d72bec2b684d704b0aa9296b1c9c/serve/models.py)
and [settings documentation](https://github.com/mw00/project-maya/blob/249f9a87d1a9d72bec2b684d704b0aa9296b1c9c/docs/SETTINGS.md).

Maya has its own `/api/models/settings`, `/api/models/switch` and `/api/context`
routes. Changing context can restart the engine after in-flight work finishes;
this is not portable conversation restoration. Maya also includes local
history/logging, optional response storage, resume buffers and update mechanisms
that need an explicit privacy review before use in this deployment. We have not
installed its new model or validated it as an k-lani-link privacy backend.
[Reviewed server](https://github.com/mw00/project-maya/blob/249f9a87d1a9d72bec2b684d704b0aa9296b1c9c/serve/server.py).

Basalt accepts common OpenAI sampling fields and some llama.cpp-style timing
names, but that does not make all flags identical. LM Studio likewise has
[its own native management API](https://lmstudio.ai/docs/developer/rest).
OpenAI-compatible chat endpoints alone do not standardize loading or KV storage.

## Mellum 2.1 via llama.cpp

`deploy/mellum.compose.yaml` runs JetBrains Mellum 2.1 (12B-A2.5B Thinking, Q8_0 GGUF)
with the official `ghcr.io/ggml-org/llama.cpp:server-cuda` image: loopback port 18091,
read-only, no capabilities, no restart policy, full 131,072-token context with flash
attention and a 4-bit KV cache. Only one model fits a 16 GB card, so stop Basalt first.
Measured 2026-10-30 on an RTX 5070 Ti (driver 595.99), image built the same day:

| | Direct | Through k-lani-link (HTTPS mailbox, local relay) |
| --- | ---: | ---: |
| Load time, VRAM | 12 s, 13.6 GB | — |
| Short code task: first token | 196 ms | 222 ms |
| Generation | 171.7 t/s | 173.0 t/s |
| 68k-token prompt (249 KB body): ingest | 6,077 t/s | 6,059 t/s |
| First token after it | 11.3 s | 11.5 s |
| Needle at 65 % depth found | yes | yes |

The tunnel adds tens of milliseconds to the first token and nothing measurable to
generation. The same server answers `/v1/messages` (Anthropic) through the tunnel, and an
unmodified Claude Code reached it there (set `CLAUDE_CODE_MAX_CONTEXT_TOKENS=131072`;
Claude Code does not know the model and otherwise assumes 200k). Thinking output arrives
as `reasoning_content` / Anthropic `thinking` blocks before the answer. One run each, a
synthetic haystack; not a quality benchmark. The model only loads on the GPU when
started; `docker compose -f deploy/mellum.compose.yaml down` frees it again.

## Context means three different things

1. **Conversation history**: messages, tool calls and tool results. A client can
   resend these to another model after validating that model's template, tools
   and context limit. Choosing not to retain history starts a new conversation.
2. **Context capacity**: the configured token budget, possibly divided among
   parallel slots. Raising it consumes RAM/VRAM; it does not load old messages.
3. **KV/recurrent state**: internal inference cache. Compatibility depends on
   weights, quantization, tokenizer, template, engine and cache format. Never
   silently restore it into a different model or call it a portable chat file.

Any future persistence should be opt-in, explicitly located and encrypted.
The default privacy mode should not create conversation archives.

## Recommended next increment

Extend the existing separately authenticated model administration with
capability-based adapters and common labels such as model, context tokens,
parallel slots, temperature, top-p, top-k, output limit, reasoning budget and KV precision. Translate only
supported settings in backend adapters; show engine-specific options as such.
Loading should accept allowlisted local model IDs, never arbitrary filesystem
paths, downloads or environment variables from an inference client.

Concurrency now uses per-request state, cancellation and flow control, with a bounded
FIFO queue. The managed Basalt adapter waits for in-flight requests before a model
change and holds later requests until it completes. The separate operator key is
not mounted in AI application containers. See the [managed deployment](managed/README.md).

The remaining roadmap includes other backend adapters and portable conversation
handling. This implementation does not save KV state or run multiple loaded model
packs at once.

**Scope.** k-lani-link stays one encrypted tunnel from one workstation to the owner's own
models, with several backends and APIs behind it. Considered and deliberately not planned
until there is demand (owner decision, 2026-10-30):

- Many-to-many deployment inside one network (many frontends and backends connecting
  directly by IP). Inside a network without a firewall between them, tools can reach
  backends directly; that is an API gateway, not this tunnel.
- Speech (`/v1/audio/transcriptions`, `/v1/audio/speech`): needs multipart uploads,
  binary responses and request content-type passthrough.
- Gemini API (Gemini CLI): the model sits in the URL path, so it needs path-pattern
  routes; its subscription terms would need the same review as Claude's.
- Asynchronous job mode ("dead drop"): an encrypted job is left on any store-and-forward
  medium (SFTP, S3-compatible storage, e-mail, Git), the model host picks it up and
  leaves the encrypted result. For long jobs that need no live dialog; it needs its own
  timing rules, because the live tunnel ends a session after 23 s without the peer.
- Relay on Cloudflare Workers with Durable Objects: waiting requests cost almost nothing
  there and there is no PHP worker limit. Cloudflare is a US provider that would see
  IP addresses and timing (never content); needs a GDPR review first.
- Streaming GET reader for the Rust relay: one long-lived response delivers records as
  they arrive (about 25 ms lower median in the carrier benchmark), with fallback to
  finite polls when a buffering proxy holds the stream back. Not for PHP hosting.
