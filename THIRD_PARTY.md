# Third-party components

| Component | Use | Source / license |
| --- | --- | --- |
| Basalt / Strata | Separate inference server; narrow serving patch | [Basalt](https://github.com/jesdga95/basalt), MIT; copy in `integrations/basalt/LICENSE` |
| ThinkRail 0.3.0-nightly.7 | Separate test front end with the bundled Pi core | [ThinkRail](https://github.com/JetBrains/thinkrail/tree/v0.3.0-nightly.7), Apache-2.0; `THIRD_PARTY_LICENSES/ThinkRail-Apache-2.0.txt` |
| Pi 1.4.0 | Agent core bundled in ThinkRail | [Pi](https://github.com/earendil-works/pi/tree/v1.4.0), MIT; `THIRD_PARTY_LICENSES/Pi-MIT.txt` |
| OpenCode 2.2.0 | Second isolated agent client | [OpenCode](https://github.com/anomalyco/opencode/tree/v2.2.0), MIT; `THIRD_PARTY_LICENSES/OpenCode-MIT.txt` |
| WordCraft 0.11.0 / GridCraft 0.12.1 | Optional local document and spreadsheet tools via MCP | [WordCraft](https://github.com/storytold/wordcraft/tree/v0.11.0), [GridCraft](https://github.com/storytold/gridcraft/tree/v0.12.1), MIT or Apache-2.0; notices in `THIRD_PARTY_LICENSES/craft/` and in the optional image |
| Rust dependencies | HTTP, WebRTC and cryptography | Versions in `Cargo.lock`; license information of the respective crates |
| Docker base images | Build and runtime environments | Official Rust, Python, NGINX, Ubuntu and NVIDIA CUDA images; their own terms apply |

ThinkRail is downloaded directly as an official release during the image build and
verified against a fixed SHA-256 checksum. Binaries and model weights are not
shipped in the source archive. The operator supplies the model weights.

The integration is an independent project and not an official JetBrains or OpenAI
release. The license status of the original code is in [LICENSING.md](LICENSING.md).
