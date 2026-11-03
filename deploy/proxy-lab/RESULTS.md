# TLS inspection: verified behavior

The reproducible Docker laboratory runs the real Linux k-lani-link client, host and
relay with a deterministic synthetic model server. Its forward proxy terminates and
inspects client TLS, then establishes separately verified TLS to the relay. The client
has no default IPv4 route and cannot reach the relay or model directly.

The machine-readable result is published alongside this page as
[results-2026-11-03.json](results-2026-11-03.json). It records each assertion, protocol
observations, binary and image hashes, and the exact laboratory source-file hashes.
The [English runner guide](README.md) and [German guide](ANLEITUNG.de.md) explain how to
repeat the experiment.

<!-- VERIFIED-RUN:START -->
**Run 2026-11-03T19:55:59.822438+00:00: 17/17 scenarios passed.** Seven policy unit tests also passed.

Environment: Linux x64, Docker 29.8.1, mitmproxy 12.3.1 (image pinned by digest).
The application was built from public source commit `0c95dd3af4f606b760d7e9dd7454e06ca1f6e770`.

| Scenario | Observed result |
| --- | --- |
| `inspected_https` | PASS |
| `pairing_by_code` | PASS |
| `domain_denied` | PASS |
| `missing_inspection_ca` | PASS |
| `wrong_origin_name` | PASS |
| `expired_origin_certificate` | PASS |
| `basic_proxy_auth` | PASS |
| `http1_only` | PASS |
| `post_only` | PASS |
| `json_only` | PASS |
| `buffered` | PASS |
| `lost_write_ack` | PASS |
| `proxy_restart` | PASS |
| `revoke_domain` | PASS |
| `revoke_token` | PASS |
| `tampered_record` | PASS |
| `blocked_quic` | PASS |

After token revocation and relay restart, the client reported access denied after 23.041 seconds. A request submitted during recovery did not execute at the model. This measures when the client observed the rejection, not an interval of continued permission.

All labelled test containers and networks were removed, and temporary test credentials were deleted.
<!-- VERIFIED-RUN:END -->

## What these results establish

- Pairing by code and a complete, ordered streamed response can pass through the tested
  TLS-inspecting forward proxy with certificate verification enabled on both TLS legs.
- An exact-domain block, a missing inspection CA, a wrong origin certificate hostname
  and an expired origin certificate are rejected in their respective scenarios.
- Basic proxy authentication and HTTP/1.1 operation work in the tested setup.
- The automatic PUT-to-POST and JSON-wrapped POST adaptations preserve the tested
  response. A separate UDP-drop case verifies automatic TCP fallback when optional
  HTTP/3 cannot connect. WebRTC-to-HTTPS transport switching is not part of these tests.
- Buffering, one lost write acknowledgement and a proxy restart are handled in the
  tested workloads without a duplicate synthetic-model execution.
- Revoking domain permission or the relay admission token prevents subsequent work
  from reaching the model. The token test also submits work during connection recovery
  and requires an explicit relay 401 plus the client's access-denied status.
- Known synthetic question and answer markers are absent from inspected bodies,
  including decoded JSON/base64 wrappers. A corrupted sealed record produces an
  authentication failure rather than an accepted answer.

## Product statement supported by this validation

> k-lani-link has been tested with a TLS-inspecting HTTPS forward proxy in a
> reproducible Docker laboratory. Pairing, streaming, HTTP compatibility fallbacks,
> connection recovery and access-revocation scenarios passed with certificate checks
> enabled. The relay and model endpoints remain subject to organizational approval.

Deutsch:

> k-lani-link wurde mit einem TLS-inspektierenden HTTPS-Ausgangsproxy in einem
> reproduzierbaren Docker-Labor getestet. Kopplung, Streaming, HTTP-Fallbacks,
> Wiederherstellung und Zugriffsentzug bestanden die Tests bei aktiver
> Zertifikatsprüfung. Die Freigabe der Relay- und Modell-Gegenstellen erfolgt
> durch die jeweilige Organisation.

## Scope

The result applies to this Linux x64 / Docker / mitmproxy configuration and the listed
workloads. The model is synthetic; these figures are not an inference-performance or
model-quality benchmark. Marker checks and rejection of a modified record are regression
evidence, not an independent cryptographic audit or security certification.

The lab's hostname allowlist deliberately simulates an administrator's allow/block
decision. It is not a commercial URL-reputation feed. Compatibility with a particular
company's gateway and policy, PAC/NTLM/Kerberos, native Windows trust-store behavior and
endpoint controls still needs validation by that company's administrators.

The inspecting proxy can see outer HTTP headers and metadata, including relay admission
tokens. Application encryption protects the model payload separately; content DLP, if
required, needs an approved endpoint before encryption or after decryption.
