# Delivery and validation status

Reviewed on 4 November 2026. Historical handovers describe their original
snapshot; they are not the current list of unfinished implementation tasks.

| Area | Delivered and checked | Boundary of the evidence |
| --- | --- | --- |
| Regular portable release | [1.2611.4](https://github.com/MarvKalani/k-lani-link/releases/tag/v1.2611.4), six platform builds, source archive and updater-signed checksums | Windows publisher signing and Apple notarization are not claimed |
| Native application | [Windows, macOS and Linux x64/ARM64 tests and builds](https://github.com/MarvKalani/k-lani-link/actions/runs/41763745185) | Native CI is not a customer-site deployment approval |
| Source and dependencies | [Release source checks and audit](https://github.com/MarvKalani/k-lani-link/actions/runs/41763742616) | Later source changes have their own Check workflow |
| Managed Basalt | Source version 0.2.1, operator-selected profiles and performance settings, no-swap load guard | Model files and hardware budgets remain operator choices |
| OpenCode Docker starter | Multiple managed models and their actual context limits; [real read/write/shell acceptance](../deploy/opencode/validation-20261104.json) | The recorded agent task used the Coder; listing another model is not a tool-quality assessment of that model |
| TLS-inspecting proxy laboratory | [17 recorded scenarios](../deploy/proxy-lab/RESULTS.md) | A real company's proxy, policy and administrator approval remain external |
| Product presentation | Current English/German wording and bidirectional request/answer relay illustration | Historical recordings and versioned download URLs retain their provenance |

The managed adapter's test suite covers model switching, privacy rejection,
operator-only performance settings and rechecking host swap before model loads.
The OpenCode tests cover both model-list formats, context validation, duplicate
IDs, alias collisions and initial selection of an already loaded model. Runtime
acceptance is recorded separately from those unit tests.

## Deliberate boundaries and follow-up choices

- The separately tested Windows MXC sandbox remains a pilot with documented
  platform and shell-tool limitations. It is not an OS-signed customer installer
  bundled with the portable release.
- Cloudflare Workers has [local workerd validation](../deploy/cloudflare-relay/README.md#validation).
  A production Cloudflare deployment is not claimed.
- Other IDE clients, backend-management adapters, portable KV state and a browser
  extension require their own acceptance work if selected. They are not implied
  by API compatibility. See [product uses and options](PRODUCT-OPTIONS.md).
- Company approval and legal/trademark clearance cannot be inferred from a code
  test or a public release. No such clearance is claimed here.

The original rename handover is preserved in [CODER-HANDOVER.md](CODER-HANDOVER.md).
Its release 1.2610.29 and machine-test notes are historical; use the release pages,
current CI and the linked acceptance records for the present technical status.
