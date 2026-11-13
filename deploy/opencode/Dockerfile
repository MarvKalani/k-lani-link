FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928 AS download
COPY deploy/opencode/download.py deploy/opencode/release.json /opt/
RUN python -B /opt/download.py /opt/release

FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928
RUN apt-get update && apt-get install -y --no-install-recommends git bash ripgrep ca-certificates libstdc++6 tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 --shell /bin/bash opencode \
    && install -d -o 1000 -g 1000 -m 0700 /home/opencode/.config/opencode /home/opencode/.local/share/opencode/log /home/opencode/.cache \
    && chown -R 1000:1000 /home/opencode && chmod 0700 /home/opencode
COPY --from=download /opt/release/opencode /usr/local/bin/opencode
COPY --from=download /opt/release/installation.json /opt/opencode-installation.json
COPY deploy/opencode/runtime.py /opt/runtime.py
COPY deploy/codex/bridge.py /opt/bridge.py
COPY deploy/desktop/prepare.py deploy/desktop/verify.py /opt/desktop/
COPY THIRD_PARTY_LICENSES/OpenCode-MIT.txt /usr/share/licenses/opencode/LICENSE
ENV PYTHONDONTWRITEBYTECODE=1 BUN_RUNTIME_TRANSPILER_CACHE_PATH=0 OPENCODE_DISABLE_MODELS_FETCH=1 \
    OPENCODE_DISABLE_PROJECT_CONFIG=1 OPENCODE_DISABLE_CLAUDE_CODE=1 OPENCODE_DISABLE_FFF=1 \
    OPENCODE_FILEWATCHER_DISABLE=1
USER 1000:1000
WORKDIR /work
RUN --network=none opencode --version
ENTRYPOINT ["/usr/bin/tini", "--", "python", "-B", "/opt/runtime.py"]
