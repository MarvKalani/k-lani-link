FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928 AS download
COPY deploy/opencode/craft-download.py deploy/opencode/craft-release.json /opt/
RUN python -B /opt/craft-download.py /opt/craft

FROM k-lani-opencode:2.2.0
USER root
RUN apt-get update && apt-get install -y --no-install-recommends fontconfig fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*
COPY --from=download /opt/craft/bin/ /usr/local/bin/
COPY --from=download /opt/craft/share/doc/ /usr/share/doc/
COPY THIRD_PARTY_LICENSES/craft/ /usr/share/doc/link-craft-notices/
COPY --from=download /opt/craft/installation.json /opt/craft-installation.json
COPY deploy/opencode/runtime.py /opt/runtime.py
ENV LINK_CRAFT_TOOLS=1 WORDCRAFT_REGION=en-GB
USER 1000:1000
RUN --network=none wordcraft-cli --version && gridcraft-cli --version
