# Security model

> A German version is in [docs/SECURITY.de.md](docs/SECURITY.de.md); if the two differ,
> this English text is authoritative.

The pairing code is brought to both devices over a channel you already trust. Its
first twelve characters form the public room identifier; 184 bits stay secret.
Whoever knows the full code can authenticate as a peer. The code is consumed only
after encrypted, mutual key confirmation. The consumption state must be kept
permanently.

Signaling and payload data are protected with XChaCha20-Poly1305 in addition to
WebRTC/DTLS. Session keys are bound to a direction; packet numbers prevent replay
and reordering. This additional pre-shared-key layer has no ephemeral
Diffie-Hellman exchange of its own. There is no plaintext fallback and no automatic
TURN service.

The intermediary can see connection metadata and can block connections. Encrypted
records are padded to fixed size buckets (1, 2, 4, 8 or 12 KiB), so their sizes do not
reveal exact token or prompt lengths; their number and timing remain visible. Its
connection is closed after the tunnel is set up. Network interruptions can be
bridged within the same session; a new tunnel needs a new code. Aborted generations
are not retried automatically.

k-lani-link offers explicitly allowed inference routes and opt-in operator-only model
control routes. It checks the
backend's privacy profile before every request. This self-declaration is not a
cryptographic attestation. Basalt must be operated with the included privacy patch
and the intended network isolation. With `allow_unverified_backend = true` the
operator explicitly switches the check off; `check` and the start then warn, and
the backend's logging behavior is entirely the operator's responsibility.

The ThinkRail lab environment restricts IPv4, IPv6 and DNS to the required local
connections. Browser access binds to loopback only and checks host and origin; a
CSP limits the page's resources to local sources. The optional resource recorder
runs on the host and reads only selected Docker statistics and system counters. The
Docker socket is not made available to any agent container.

A compromised endpoint, other local programs, deliberately released tools and host
administration are outside the tunnel encryption. This MVP has no independent
security audit or GDPR certification yet.

## Reporting a problem

Contact **support@kalanis.de**. Please use synthetic reproductions and remove keys,
prompts and personal data.

## WebRTC advisory review — 28 October 2026

The native dependency is the Rust `webrtc` crate **0.21.1**, pinned in Cargo.lock.
This runtime does not link Neko, Signal RingRTC or Chromium's libwebrtc. Updating
a browser does not change that Rust transport; a future browser adapter would
need its own browser-version policy.

| Report | Affected component / fix | Relevance here |
| --- | --- | --- |
| Neko CVE-2026-116679 / GHSA-5cp7-fp4j-wwcv | Neko 3.0.0–3.1.4; fixed in 3.1.5. Legacy WebRTC input authorization bypass under the documented settings. | Neko is not used. This is an application authorization defect, not a general break of WebRTC encryption. |
| Signal RingRTC CVE-2026-113380 | RingRTC before 2.72.0. Saturating the FSM queue can prevent local hang-up. | RingRTC is not used. We added a real encrypted DataChannel regression for local close and Drop while the application receive queue is full. |

Primary sources: [Neko advisory](https://github.com/m1k1o/neko/security/advisories/GHSA-5cp7-fp4j-wwcv),
[RingRTC fix](https://github.com/signalapp/ringrtc/commit/4b1b6fa53c9f30c87c8ea78e44675d4a13e1e4d3),
[CVE record](https://github.com/CVEProject/cvelistV5/blob/main/cves/2026/113xxx/CVE-2026-113380.json).
The incomplete identifier `CVE-20217951` could not be confidently matched.

`cargo audit` with the RustSec database at commit
`99670cfc0bc574433034b75876a4b791ccaa2efb` (updated 27 October 2026) reported no
known vulnerable dependencies or warnings for this lockfile. This is not proof
that the code is free of vulnerabilities. The queue regression completed with
explicit close under two seconds and remote closure under three seconds; it
also checks Drop and blocked sender termination. Existing tests cover tampering,
replay/reflection, mismatched keys, consumed codes, relay rejection, stream
cancellation and signaling disconnection. Generic backend administration routes remain denied. The two `/v1/link/` model
control routes require explicit host opt-in, a separate client-side operator key
and a private Unix supervisor socket. Only locally allowlisted IDs are accepted;
AI applications are not given the operator key or Docker socket.

The two named advisories did not require a production CVE patch in k-lani-link.
We do not describe an unrelated dependency upgrade as fixing them. Recheck
the lockfile and relevant advisories before each release.

## Optional measurements and intentional output paths

The Basalt numeric exporter is explicitly opt-in and writes only an allowlisted
counter snapshot to tmpfs. The host sampler can record CPU, RAM, GPU and token
timings when asked; it does not capture inference content. This measurement
exception is separate from disabled application logs and request history.

Both AI application containers use `network_mode: none`; the k-lani-link transport
containers deliberately have network access to establish ICE and reach the peer.
They are not containers with all egress removed. The local browser, Unix sockets,
mounted workspaces and exported documents are intentional data paths. The setup
therefore makes no absolute assertion that data cannot leave two containers.
Purchase pages are separate browser visits and have no runtime activation path.
