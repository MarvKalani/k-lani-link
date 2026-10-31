# Recorded local AI workflow

[Watch / download the English MP4](https://github.com/MarvKalani/k-lani-link/releases/download/v1.2610.29/k-lani-link-business-demo.mp4)

An 87-second **edited archive**, published on 29 October 2026, of the real
OpenCode/Basalt business workflow recorded on 27–28 October. It shows streamed
reasoning, local document tools, operator review, corrections, and the original
fictional invoice and ledger. It has no audio. It is not a fresh model run.

- [Streaming GIF](k-lani-link-streaming.gif): ten seconds of recorded reasoning
  and container counters, looping at the end.
- [Document tools GIF](k-lani-link-document-tools.gif): eight seconds of the
  invoice tool workflow, looping at the end.
- [Original invoice PDF](../../website/examples/invoice.pdf)
- [Original formula-based XLSX](../../website/examples/ledger.xlsx)

## What was edited

The browser titles, former product artwork, old title overlays and former provider
name badges are covered. New English title cards, captions and a resource legend
explain the run. Selected model output and counters are not rewritten. The original
elapsed-time labels are retained; jumps between excerpts are edits, not a claim
that the whole task completed in 87 seconds. Captures are timestamped screenshots,
not a continuous high-frame-rate screen recording. The export uses four frames
per second and does not speed up the selected excerpts.

The opening and closing cards were written by Codex. Codex also operated the
original run: setup, prompts, follow-ups, review and video assembly. Basalt through
OpenCode created and corrected the documents. They remain byte-identical to the
accepted output. The old website-generation footage stays private because the
former name appears throughout its prompts and code. The current website has
[disclosed later operator edits](../../website/README.md).

Both peers ran on one machine, TURN was disabled and signaling stopped after
pairing. All customer information is fictional. The recording contains real
CPU/RAM/GPU counters, **not measured token/s**. The model was the local
Qwen3.8-Flash-Next IQ3_XXS Coder pack, not NVFP4. The 262,144-token capacity was
configured, not filled. This is not a WAN, full-context or throughput benchmark.
[Hardware, OS, Docker and measurement definitions](../../deploy/HARDWARE.md).

Codex could see synthetic screenshots. Container network isolation does not
establish a cloud-free operator desktop or automatic GDPR compliance.

## Publication and integrity

The MP4 is an additional media asset on the existing portable preview release;
it does not replace or rebuild its executables. `DEMO-SHA256SUMS` on that release
covers the video and both GIFs separately from the original binary checksums.
The GIFs are also stored here for inline GitHub README playback. GitHub's image
proxy may cache previews; the MP4 is the higher-resolution version.

Video assembly/redaction uses FFmpeg. GIFs are exported with **FilmCraft 0.13.0's
headless CLI in a network-disabled container**, then optimized with gifsicle.
Color reduction in GIFs is lossy. `manifest.json` records durations, sizes and
SHA-256 digests. Private full recordings, runtime state and capture archives are
not included. The downloadable source bundle excludes the large GIFs.
