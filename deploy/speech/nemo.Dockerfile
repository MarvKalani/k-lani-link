FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS release
ADD --checksum=sha256:255f12b722ab15b2a38a51cf125d56e3fa97058b528f7e895b39d6a473deb06e \
    https://github.com/NVIDIA/NeMo-Speech.cpp/releases/download/v0.3.0/nemo-speech-0.3.0-linux-x86_64-cpu.tar.gz /tmp/nemo.tar.gz
RUN mkdir /opt/nemo && tar -xzf /tmp/nemo.tar.gz --strip-components=1 -C /opt/nemo

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN apt-get update && apt-get install -y --no-install-recommends socat \
    && rm -rf /var/lib/apt/lists/*
COPY --from=release /opt/nemo /opt/nemo
COPY deploy/speech/nemo-start.sh /usr/local/bin/nemo-start
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md THIRD_PARTY.md /usr/share/doc/k-lani-link/
ENV HOME=/tmp
USER 1000:1000
ENTRYPOINT ["/bin/sh", "/usr/local/bin/nemo-start"]
