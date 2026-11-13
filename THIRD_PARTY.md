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
| Audio8-TTS-Realistic-Air | Optional speech synthesis | [Code and weights](https://huggingface.co/Edge0/Audio8-TTS-Realistic-Air), Apache-2.0 with upstream `NOTICE`; operator-supplied |
| Ming 3.0 TTS 0.5B | Optional speech synthesis | [Inference code](https://github.com/inclusionAI/Ming-3.0-tts) and [weights](https://huggingface.co/inclusionAI/Ming-3.0-tts-0.5B), MIT; operator-supplied |
| NeMo-Speech.cpp 0.3.0 | CPU runtime for the optional speech Compose profile | [NVIDIA release](https://github.com/NVIDIA/NeMo-Speech.cpp/releases/tag/v0.3.0), Apache-2.0; licenses and notices in the release archive |
| Nemotron 3.5 ASR Streaming 0.6B | Optional file transcription | [Model and GGUF](https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b), OpenMDW-1.1; operator-supplied |

ThinkRail is downloaded directly as an official release during the image build and
verified against a fixed SHA-256 checksum. Binaries and model weights are not
shipped in the source archive. The operator supplies the model weights.

The integration is an independent project and not an official JetBrains or OpenAI
release. The license status of the original code is in [LICENSING.md](LICENSING.md).
