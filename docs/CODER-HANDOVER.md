# Handover to the maintainer: what changed after the snapshot, and why

Written on 28 October 2026 for whoever continues this code, human or coder. Read this
before you touch names, protocol constants or the local API's upload handling. It is
the only file in the repository allowed to spell the former product name; everything
else is checked against it by `deploy/check-names.py`.

## Why this exists

The product was renamed to **k-lani-link** because the former name collides with an
existing company's mark. The owner's goal is simple and strict: **nothing a buyer can
read, type, capture or download may carry the former name any more.** The first rename
changed the product, the binary, the packages and the artwork, but deliberately kept
many internal identifiers "for compatibility". A review of the published snapshot
found that this compatibility protected nobody:

- the first release's assets showed no downloads when last checked, and sales are paused;
- the only existing installations are the owner's own labs;
- a rename later would be a protocol change, so now is the cheapest moment there will be.

The same review found two weaknesses in the new multiplexing code. This change fixes
both and renames what was left. Nothing else is meant to differ from your snapshot.

## 1. Behaviour changes

### The local API no longer holds a slot for a stalled upload

Before: `proxy` in `src/gateway.rs` took a capacity slot, then read the request body
with the whole request timeout (600 s by default) as its only limit. A local client that
stopped sending kept its slot for up to ten minutes, while the host already refuses
uploads that take longer than 30 s. With the default of one stream, that was a complete
outage caused by one stuck client.

Now: both sides share `UPLOAD_SECONDS` (30) from the protocol crate. A body that has not
fully arrived by then ends with **HTTP 408** and returns the slot. `Gateway::with_upload_timeout`
exists for tests only. The regression test is
`a_stalled_upload_gives_its_slot_back_after_the_upload_limit` in `tests/multiplex.rs`;
it fails without the limit.

Known limit, not changed: 30 s is generous on loopback but tight for an 8 MiB prompt over
a slow uplink. If users report 408 on large requests, make the limit a configuration
value rather than raising the constant.

### The operator key reports what is wrong

Before: any problem with `management_token_file` produced one sentence. A key file owned
by another user (typical when a container mounts a host file) looked exactly like a key
that was too short.

Now `src/management.rs` returns one fixed reason per problem: not found, not readable by
this user, not a regular file (link or directory), larger than 4 KiB, open to group or
others, wrong length or characters, or identical to the API key. No reason repeats key
material, and the unit test asserts that.

## 2. Renamed identifiers

Wire format and behaviour are otherwise identical. Only names changed: every identifier
below now carries the product name. The former names are not recorded in this repository.

| Area | Now |
| --- | --- |
| Environment variables | `LINK_HOST_CONFIG`, `LINK_CLIENT_CONFIG`, `LINK_API_KEY`, `LINK_API_SOCKET`, `LINK_API_PORT`, `LINK_API_VOLUME`, `LINK_NUMERIC_METRICS`, `LINK_REQUIRE_NO_SWAP`, `LINK_STATS_PORT`, `LINK_CRAFT_TOOLS`, `LINK_TEST_MODEL`, `LINK_LIVE_SIGNAL_URL` |
| Backend health field | `link_privacy` |
| API error type | `link_error` |
| Container paths | `/run/link`, `/run/link-api`, `/run/link-metrics`, `/var/lib/link`, `/home/link`, `/opt/link-*`, `/tmp/link-*.patch` |
| Container user, Compose service | user `link`, service `link-host` |
| Pairing-state directory | `.link-key-state` |
| Provider ID and files | provider `link`, `link/basalt`, `link.config.toml`, `link.config.models.json` |
| Lab script options | `--link-root`, `--link-image` |
| DataChannel label | `link-v1` |
| Key-derivation and one-time-code domain strings | `k-lani-link/session/v2`, `…/payload/v2/…`, `…/sealed-sdp/v2`, `…/one-time/v2` |
| SDP carrier | `s=k-lani-link`, `a=x-k-lani-link-v2:` |

The Basalt patches (`privacy.patch`, `numeric-metrics.patch`) were edited only on added
lines, so they still apply to the pinned commit `71116cc3`; the health field in the patched
server is now `link_privacy`. `src/backend.rs`, `deploy/managed/manager.py`, the mocks in the
tests and every document agree with it.

### What this breaks, on purpose

- **Old and new builds cannot pair.** The domain strings feed the key derivation, so a
  mismatch fails closed with "pairing rejected". There is no fallback and no downgrade.
- **Every earlier pairing code is void.** The consumed-code markers are hashed with the
  new domain string, so a code that was consumed before looks unused now. Never reuse
  one. Create a fresh pair for each deployment, and do not delete `.used` or `.reserved`
  files to make a code work.
- **Generated lab state is stale.** `prepare.py` writes `compose.env` and TOML files with
  the old variable names. Regenerate every lab; do not edit the old files by hand.
- **Anything outside the repository still uses the old names:** shell aliases, systemd
  units, private backups, saved Compose files. Update them or leave them alone, but do not
  copy snippets from the private archive back into this repository.

## 3. A guard so it stays fixed

`deploy/check-names.py` runs in CI (job `source`). It fails when the former name appears in
any tracked or new file, in a file name, or in a patch, except in this document and in
`THIRD_PARTY_LICENSES/`. The pattern is built from two halves so the script does not match
itself. Do not add allowlist entries to get a change through; rename the identifier. When
you must describe history, say "the former product name" the way the other documents do.

## 4. What was verified, and what was not

Verified here: formatting, Clippy with warnings denied, the whole Rust test suite (including
the new upload test and the shutdown, multiplex and e2e tests), the WASM check, the three
Python test suites, the website check, the name guard, the source bundle, and that every
script, JSON, TOML and YAML file parses. Both Basalt patches apply to `71116cc3` and the
patched Python files parse.

**Not verified here, please run on the real machine:**

1. Basalt's own unit tests (`serve.tests.test_privacy`, `serve.tests.test_numeric_metrics`).
   They run during the Docker build of the backend image; a failing build means the rename
   broke the patch contract.
2. `deploy/smoke.py`, `deploy/codex/smoke.py`, `deploy/managed/smoke.py` and the OpenCode and
   ThinkRail `verify.py`. They need Docker, and the managed one needs the GPU. This session
   had neither. Expect to rebuild all images, because the user, paths and the health field
   changed together.
3. The portable builds on Windows and macOS. The changes are platform-neutral, but only the
   Linux build was run here.

## 5. Left open on purpose

- Tag and release **1.2610.29**. The changelog heading still says "unreleased"; set the
  date when you tag, and merge the branch with a merge commit or fast-forward, never a
  squash, so the tagged commit stays reachable.
- A trademark search for **k-lani-link** (DPMA, EUIPO, TMview; classes 9 and 42) before
  checkout reopens. `docs/REBRANDING.md` correctly claims no clearance; the name "Link" alone
  is crowded, so never use it without the prefix.
- Places this repository cannot fix: the public Actions run history still shows old titles,
  the payment product's name, other pages and search or archive caches, and old commit
  hashes that GitHub keeps until it cleans them up.
- Review of the purchase-terms drafts by a qualified person. Until then they are drafts, and
  they contain the owner's postal address.
