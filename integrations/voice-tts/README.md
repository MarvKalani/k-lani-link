# Audio8 and Ming 3.0 through k-lani-link

The adapter serves Audio8-TTS-Realistic-Air and Ming 3.0 TTS 0.5B through
`POST /v1/audio/speech`. Both use the same operator-provided voice reference.
The requested `model` selects the engine. A worker loads the selected engine
on CUDA for each request and releases GPU memory when it exits. Requests that
arrive during another clone synthesis receive HTTP 429.

For container deployment and optional client-local routing, see the
[speech Compose profile](../../deploy/speech/README.md).

## Local configuration

Install Bubblewrap and a Python environment with CUDA-enabled PyTorch,
Transformers, SoundFile and the dependencies from the
[Ming inference repository](https://github.com/inclusionAI/Ming-3.0-tts).
Use locally installed model files. The worker uses a network namespace and
reads the model and reference files from read-only mounts.

Use a private directory outside the checkout for `reference.wav`,
`reference.txt` with its matching transcript, and `model.json`. The
configuration supplies three absolute directory paths.

```json
{
  "audio8_model": "/absolute/path/to/Audio8-TTS-Realistic-Air",
  "ming_model": "/absolute/path/to/Ming-3.0-tts-0.5B",
  "ming_source": "/absolute/path/to/Ming-3.0-tts"
}
```

Keep each backend key in a separate file readable by the operator only.
Use a different private directory for the worker cache. Start the
[NeMo service](../nemo-speech/README.md) for Nemotron STT and Magpie TTS.

```sh
python3 integrations/voice-tts/serve.py \
  --voice-dir "$VOICE_DIR" --state-dir "$VOICE_STATE" \
  --worker-python "$TTS_PYTHON" \
  --api-key-file "$VOICE_KEY_FILE" --nemo-key-file "$NEMO_KEY_FILE"
```

The adapter listens on loopback port 8192 and forwards `magpietts` to
NeMo on 8191. In the private Link host configuration, put `speech = true`
on the 8192 backend and `transcriptions = true` on the 8191 backend.
Set each backend's `upstream_api_key` in that same private configuration.
Use `allow_unverified_backend = true` on these local services to use their
own runtime privacy settings.

On a machine with one shared NVIDIA GPU, `--comfy-port 8188` lets the
adapter release ComfyUI's idle cache when less than 4 GiB is available.
If ComfyUI has queued or running work, the audio request receives HTTP 503.

For a restartable installation, run the adapter and NeMo under a service
manager. The Link's HTTPS pairing can reconnect from saved state when the
host and client restart. Start a local relay on the same machine to keep
the whole installation local.

## Use the client gateway

Send the usual API key to your Link gateway. Choose either model ID with
the local voice alias. This profile returns PCM WAV and uses German for
Ming. Each request accepts up to 4,096 characters. Keep the output in a
private local directory.

```sh
curl --fail --silent --show-error "$LINK_BASE_URL/audio/speech" \
  -H "Authorization: Bearer $LINK_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"ming-3.0-tts-0.5b","voice":"local-reference","input":"Guten Morgen.","response_format":"wav"}' \
  -o "$VOICE_OUTPUT"
```

Use `audio8-tts-realistic-air` in the same request to switch to Audio8.
