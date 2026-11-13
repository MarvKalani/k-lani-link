# NeMo-Speech.cpp with k-lani-link

NeMo-Speech.cpp serves text-to-speech and file transcription through one local
HTTP listener. k-lani-link forwards both OpenAI-compatible endpoints directly.
The tested setup uses NeMo-Speech.cpp 0.3.0, Magpie Multilingual and Nemotron
3.5 ASR Streaming 0.6B.

The [speech Compose profile](../../deploy/speech/README.md) runs Nemotron for
transcription alongside the Audio8 and Ming service.

## Start the speech server

Install the appropriate [NVIDIA release](https://github.com/NVIDIA/NeMo-Speech.cpp/releases/tag/v0.3.0).
Download the models before serving requests.

```sh
nemo-speech pull magpie
nemo-speech pull nemotron-3.5
nemo-speech serve --host 127.0.0.1 --port 8191 \
  --asr-model nemotron-3.5 --tts-model magpie \
  --tts.language-code de-DE --max-upload-mb 8 --no-ui
```

NeMo defaults to port 8080. This example uses 8191 beside ComfyUI on 8188,
its adapter on 8190 and the k-lani-link client gateway, which defaults to 8787.
Run the speech server and the link host where they share the loopback listener.
For the CPU build, use `--device cpu`. The CUDA build supports `--device cuda`.

If the speech server has its own API key, supply it to NeMo through
`NEMO_SPEECH_HTTP_API_KEY` and set `upstream_api_key` in the private host config.
The application uses the k-lani-link key at the client gateway.
[NeMo server options](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/v0.3.0/docs/server.md)

## Configure the host

Add the speech server alongside your existing backend in the host's private
configuration. With a dedicated speech pairing, use the two common settings
`upstream` and `allow_unverified_backend` at the top level.

```toml
[[backends]]
upstream = "http://127.0.0.1:8191"
allow_unverified_backend = true
speech = true
transcriptions = true
```

Each audio endpoint can have one designated backend. For separate services,
place `speech = true` on the TTS entry and `transcriptions = true` on the STT
entry. Without a designated service, that route uses the primary upstream.
On a client with several connected model computers, use the gateway for the
pairing that hosts the speech service. The opt-in uses NeMo's own runtime and
logging settings. Run the updated k-lani-link build at both ends.

## German round trip

Set `LINK_API_KEY` in your private environment. Model IDs come from
`/v1/models`, and the transcription ID can depend on the local checkpoint.
Keep audio output in the Git-ignored `.artifacts/` directory.

```sh
base=http://127.0.0.1:8787/v1
mkdir -p .artifacts/speech
models=$(curl --fail --silent --show-error "$base/models" \
  -H "Authorization: Bearer $LINK_API_KEY")
tts_model=$(printf '%s' "$models" | jq -er '.data[] | select(.capability == "speech") | .id')
asr_model=$(printf '%s' "$models" | jq -er '.data[] | select(.capability == "transcription") | .id')

jq -n --arg model "$tts_model" \
  '{model:$model, input:"Über dem stillen See leuchtet der Vollmond.", voice:"Sofia", response_format:"wav"}' |
  curl --fail --silent --show-error "$base/audio/speech" \
    -H "Authorization: Bearer $LINK_API_KEY" -H 'Content-Type: application/json' \
    --data-binary @- -o .artifacts/speech/deutsch.wav

curl --fail --silent --show-error "$base/audio/transcriptions" \
  -H "Authorization: Bearer $LINK_API_KEY" \
  --form-string "model=$asr_model" --form-string 'language=de' \
  -F 'file=@.artifacts/speech/deutsch.wav;type=audio/wav'
```

The link carries the multipart form and its boundary for transcription and
streams response bytes with the backend's content type. The total upload
limit is 8 MiB. The example uses file-based STT and one TTS request per utterance.
[NeMo HTTP API](https://github.com/NVIDIA/NeMo-Speech.cpp/blob/v0.3.0/docs/api.md)

The [Audio8 and Ming adapter](../voice-tts/README.md) adds a local voice
profile through the same speech route while using NeMo for transcription.
