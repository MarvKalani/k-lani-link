# Product uses and next options

Research reviewed on 4 November 2026. This separates existing capabilities from
possible integrations; an application's API documentation is not an acceptance test.

## The problem the product solves

A person may be allowed to use a private model for confidential work while public
AI services are prohibited. Their model computer and work computer can sit on
different networks. k-lani-link carries the permitted AI API traffic through an
encrypted connection; the model stays on the operator's hardware. An HTTPS relay
can use an approved domain. An organization's policy must permit this arrangement.

The relay transports encrypted requests and answers. Application tools, project
files and document editors stay where the application runs. The tunnel itself
does not provide remote desktop or arbitrary remote file access.

## Useful workflows

| Workflow | Available path | Evidence / remaining check |
| --- | --- | --- |
| Coding in an isolated project folder | OpenCode and the Docker application profile | [Existing agent and isolation checks](../deploy/opencode/README.md) |
| Private chat and deliberate conversation saving | Native control panel | [Portable application guide](../deploy/portable/README.md) |
| Documents and local automation | An API-compatible application with local tools; tools receive only their own granted file access | Validate the chosen document tool separately; API transport alone does not prove document quality or file isolation |
| IDE assistance | Continue or Cline, configured with the local API URL, key and model ID | Provider configuration is documented upstream; these are not newly qualified k-lani-link deployments |
| Classification, extraction and batch processing | A local script calling the allowed chat endpoints | Keep retries and job persistence in the application; a disconnected live tunnel is not a durable job queue |
| Company-managed access | An approved relay domain, trusted proxy CA and documented access policy | [Reproducible proxy laboratory](../deploy/proxy-lab/RESULTS.md); actual company approval remains external |

[OpenCode documents custom providers](https://opencode.ai/docs/providers/#custom-provider).
[Continue supports a custom `apiBase`](https://docs.continue.dev/customize/model-providers/top-level/openai),
and [Cline documents the URL, API key and model fields](https://docs.cline.bot/provider-config/openai-compatible).
Client telemetry, extensions and network tools remain separate from inference
transport. For sensitive files, an enforced network and filesystem boundary is
stronger than an application's telemetry preference alone.

## Basalt performance options

Basalt recommends resident experts and automatic prefill in its
[configuration](https://github.com/jesdga95/basalt/blob/main/docs/configuration/index.md).
RAM must cover the CPU expert tier and runtime buffers. A small container limit
can force repeated file reads even when the GPU has enough VRAM.

[Basalt's probe](https://github.com/jesdga95/basalt/blob/main/docs/probe.md) measures
CPU worker counts, prefill CPU/GPU sharing and adaptive-cache settings on the
actual machine. Its recommendations are measurements, not universal defaults.
The [managed adapter](../deploy/managed/README.md#basalt-performance-settings)
exposes the supported operator settings without accepting arbitrary remote flags.

The [official conversion kit](https://huggingface.co/jesdga/Qwen3.8-Flash-Next-Basalt)
contains a tuned MTP drafter. Custom packs may lack it. Adding a compatible drafter
requires checking tokenizer/geometry, preserving target weights, rebuilding the
format's auxiliary tables and testing the resulting file before replacing a
working model. A speculation flag by itself cannot supply missing weights.

For comparisons, pin actual expert placement when checking greedy identity:
the requested cache size can be reduced by the engine when VRAM is tight. Record
the final placement, power limit, input/output counts and cache reuse. Compare
complete application tasks as well as tokens per second.

## Options to pursue only with a concrete need

1. Qualify additional IDE/application profiles with a real tool-use task and an
   egress test. Start with the application a prospective customer already uses.
2. Package the existing relay and proxy-lab instructions into a customer pilot:
   approved domain, proxy CA, allowed routes, revocation and reconnection checks.
3. Add another backend-management adapter when a customer needs that engine.
   Generic API forwarding already exists; lifecycle controls and privacy guarantees
   need engine-specific verification.
4. Consider portable conversation export or a browser extension only for a clear
   workflow gap. Engine KV state is model/version-specific and must not be treated
   as a portable chat file. Persistence must remain an explicit choice.

These are product options, not claims that the integrations or external approvals
already exist. Audio, many-to-many routing and durable asynchronous jobs remain
outside the current scope described in [backend capabilities](../deploy/BACKENDS.md).
