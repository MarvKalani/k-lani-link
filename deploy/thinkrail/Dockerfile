FROM python:3.12-slim@sha256:f77ac9e44ae96ef2c90b8053ea08c31f8be030f824196b0ae4db6d462c84e51f AS installer
COPY deploy/thinkrail/download.py /opt/download.py
COPY deploy/thinkrail/release.json /opt/release.json
RUN python -B /opt/download.py /opt/thinkrail-install

FROM python:3.12-slim@sha256:f77ac9e44ae96ef2c90b8053ea08c31f8be030f824196b0ae4db6d462c84e51f
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
