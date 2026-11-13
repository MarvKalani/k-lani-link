# MiniMax H3 with ComfyUI

Run the adapter on the model computer next to a running ComfyUI instance.
The adapter needs Python 3 and its standard library. The bundled image and
video workflows use the native ComfyUI nodes and the configured Qwen text
encoder. The workflow JSON files list the required model filenames. Edit
their loader inputs if your local files have different names.

```sh
python3 integrations/comfyui/serve.py
```

The adapter listens on `127.0.0.1:8190` and connects to ComfyUI on
`127.0.0.1:8188`. Use `--port` and `--comfy-port` for other local ports.

In the k-lani-link host configuration, add the adapter alongside your existing
backend. The host selects it when the request names `minimax-h3-image`.

```toml
[[backends]]
upstream = "http://127.0.0.1:8190"
allow_unverified_backend = true
video = true
```

The `video` flag selects the one additional backend for video creation,
status and downloads. For a dedicated pairing, put `upstream` and
`allow_unverified_backend` at the top level. Video requests use that primary
backend when no additional one has `video = true`.
Run the updated k-lani-link build at both ends. The adapter and ComfyUI are
reachable on loopback from the host process. With a containerized host, make
the adapter reachable from that container or run the host on the model computer.
Use the client gateway for the chosen pairing when several model computers
are connected. The adapter uses ComfyUI's history and temporary media files.
The opt-in reflects that ComfyUI has its own storage and privacy settings.

## Generate an image through the link

Use the client gateway and its k-lani-link API key. The example uses the
default client port and saves the decoded response to `vollmond.png`.

```sh
curl --fail --silent --show-error http://127.0.0.1:8787/v1/images/generations \
  -H "Authorization: Bearer $LINK_API_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"minimax-h3-image","prompt":"A realistic telescope photograph of the full Moon, its entire disk centered against a black sky","size":"1024x1024"}' |
  jq -er '.data[0].b64_json' | base64 --decode > vollmond.png
```

The endpoint returns one PNG per request. The size defaults to `1024x1024`.
Each side can be 256 to 2048 pixels in multiples of 32. The optional `seed`
sets the sampler seed. `response_format` and `output_format` default to
`b64_json` and `png`. A request that arrives during another image generation
gets HTTP 429.

## Animate an image through the link

The `minimax-h3-video` workflow uses the FL2VA model, the installed four-step
Turbo LoRA and the video and audio VAEs. It takes an uploaded PNG or JPEG
as its first frame and returns H.264 MP4 with stereo audio at 24 fps.

```sh
base=http://127.0.0.1:8787/v1
job_id=$(curl --fail --silent --show-error "$base/videos" \
  -H "Authorization: Bearer $LINK_API_KEY" \
  --form-string 'model=minimax-h3-video' \
  --form-string 'prompt=The full Moon stays centered during a slow telescope push-in.' \
  --form-string 'size=480x480' \
  --form-string 'seconds=4' \
  -F 'input_reference=@vollmond.png;type=image/png' |
  jq -er '.id')

curl --fail --silent --show-error "$base/videos/$job_id" \
  -H "Authorization: Bearer $LINK_API_KEY"

# Run after status is completed.
curl --fail --silent --show-error "$base/videos/$job_id/content" \
  -H "Authorization: Bearer $LINK_API_KEY" -o vollmond-video.mp4
```

Creation returns a job with status `queued`. Poll for `in_progress`,
`completed` or `failed`. Durations are 4, 8 and 12 seconds. The workflow
renders on H3's frame grid and trims to the requested duration. The default
canvas is `480x480`. Each side is a multiple of 32 between 256 and 1344, with
at most 1344 × 768 pixels total. The complete multipart upload is limited to
8 MiB. The optional `seed` sets the sampler seed.

The adapter keeps job IDs in memory for up to an hour. Downloads remain
available across tunnel reconnections while the adapter and ComfyUI retain
the job and its temporary media. Keep local API keys, pairing files and
test media in an ignored directory such as `.artifacts/`.
