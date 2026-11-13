FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928 AS installer
COPY deploy/thinkrail/download.py /opt/download.py
COPY deploy/thinkrail/release.json /opt/release.json
RUN python -B /opt/download.py /opt/thinkrail-install

FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928
RUN apt-get update && apt-get install -y --no-install-recommends git bash ripgrep ca-certificates libstdc++6 tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 --shell /bin/bash thinkrail \
    && install -d -o 1000 -g 1000 -m 0700 /home/thinkrail/.thinkrail/logs /home/thinkrail/.pi/agent /home/thinkrail/.cache \
    && chown -R 1000:1000 /home/thinkrail && chmod 0700 /home/thinkrail /home/thinkrail/.thinkrail /home/thinkrail/.pi
COPY --from=installer /opt/thinkrail-install/bin/thinkrail /usr/local/bin/thinkrail
COPY --from=installer --chown=1000:1000 /opt/thinkrail-install/installation.json /opt/thinkrail-installation.json
COPY deploy/thinkrail/runtime.py /opt/runtime.py
COPY deploy/codex/bridge.py /opt/bridge.py
COPY THIRD_PARTY_LICENSES /usr/share/licenses/link-integrations
ENV PI_OFFLINE=1 THINKRAIL_NO_ANALYTICS=1 THINKRAIL_LOG_LEVEL=error PYTHONDONTWRITEBYTECODE=1
USER 1000:1000
WORKDIR /work/demo
ENTRYPOINT ["/usr/bin/tini", "--", "python", "-B", "/opt/runtime.py"]
