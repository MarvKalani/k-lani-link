FROM python:3.15-slim-trixie@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928 AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg libsndfile1 libgomp1 \
    && rm -rf /var/lib/apt/lists/*
COPY deploy/speech/tts-requirements.txt /tmp/tts-requirements.txt
RUN python -m pip install --no-cache-dir \
      --extra-index-url https://download.pytorch.org/whl/cu130 \
      -r /tmp/tts-requirements.txt

FROM runtime
WORKDIR /opt/link-speech
COPY integrations/voice-tts/serve.py integrations/voice-tts/synthesize.py ./
COPY deploy/speech/tts-models.json ./models.json
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md THIRD_PARTY.md /usr/share/doc/k-lani-link/
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
USER 1000:1000
ENTRYPOINT ["python", "-B", "/opt/link-speech/serve.py"]
CMD ["--unix-socket", "/run/speech/tts.sock", "--voice-dir", "/voice", "--model-config", "/opt/link-speech/models.json", "--state-dir", "/tmp/voice-worker", "--worker-python", "/usr/local/bin/python"]
