# Name change and existing installations

The product, executable, Rust packages, new Docker image tags and repository are
now named **k-lani-link**. The latest reviewed development snapshot includes the
three follow-up commits through `5e67e23` before the history was consolidated.
The old Git history, release assets and original demonstrations are backed up
privately. The source repository is public again under the new name; checkout
remains paused while the purchase flow and terms are prepared.
Existing version 1.x licenses remain valid; price and free-use thresholds do not change.

## Identifiers

An earlier snapshot of the rename kept the former name in internal identifiers for
compatibility. That was dropped before the first public release, because no outside
installation existed that it could have protected. Everything a buyer can see, type
or capture now uses the new name: environment variables, the backend health field,
the API error type, container paths, user and service names, provider IDs in the
examples, pairing-state directory, lab script options, and the protocol labels
(DataChannel label, key-derivation and one-time-code domain strings, SDP carrier).
The current identifiers are listed in [CODER-HANDOVER.md](CODER-HANDOVER.md); the
former names are not recorded in this repository, its history or its file metadata.

Consequences:

- **Not compatible with older builds.** A host and a client must both be built from
  this version or later. A mismatch fails closed (the pairing is rejected), it never
  falls back.
- **Every older pairing code is void.** The one-time-code markers are derived with the
  new domain string, so earlier consumed codes are no longer recognised as consumed.
  Do not reuse them: create a fresh pair on every deployment. Do not remove `.used` or
  `.reserved` files to force a code to work.
- **Docker deployments must be rebuilt.** Images, volumes' mount paths and the
  backend-side health field changed together. Rebuild the images and re-run the
  smoke tests before relying on an existing lab.
- **A guard keeps it that way.** `deploy/check-names.py` runs in CI and fails when the
  former name reappears in any file, file name, archive member or binary.

## Demonstration provenance

Unedited videos and GIFs carrying the earlier name remain in the private archive.
The [public archive edit](../assets/demo/README.md) removes the former overlays
and provider labels and explicitly identifies the historical run and operator's
role. It is not presented as a fresh model run. Current website provenance
identifies the later operator changes to product wording and artwork. The original
invoice and spreadsheet files remain unchanged.

## Publication

No replacement public release is created by this rename. A new public release must
use this name in its downloads, current artwork and checkout. A name change alone
is not trademark clearance; no such clearance is claimed for the new name.

The public Git history is consolidated into one snapshot commit at the owner's
request. [CHANGELOG.md](../CHANGELOG.md) retains the documented feature history;
the original Git history is preserved only in private backups. Later feature
commits build on that snapshot. Version 1.2610.29 introduces portable preview
packages; their signature status and limits are in the
[portable guide](../deploy/portable/README.md).
