# llama.cpp for GGUF models in the managed catalog (deploy/managed). Pinned by digest; it
# brings its own CUDA 12 libraries, which only its process loads.
FROM ghcr.io/ggml-org/llama.cpp@sha256:f25bfa096802cdf35ceb7d124816387a1e3ed5cbfc869eb242246cd29ab29352 AS llama
RUN mkdir -p /opt/llama/lib && cp -a /app/. /opt/llama/ \
    && cp -L /usr/local/cuda/lib64/libcudart.so.12 /usr/local/cuda/lib64/libcublas.so.12 \
       /usr/local/cuda/lib64/libcublasLt.so.12 /lib/x86_64-linux-gnu/libnccl.so.2 /opt/llama/lib/ \
    && rm -f /opt/llama/llama /opt/llama/*bench* /opt/llama/*perplexity* /opt/llama/*quantize*

# The supervisor, entrypoint and health probe: k-lani-link itself, statically linked so it
# runs on the CUDA image's own libc. No interpreter of ours ships in the image.
FROM rust:1.99.1-slim-trixie AS link
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools cmake g++ pkg-config \
    && rm -rf /var/lib/apt/lists/* && rustup target add x86_64-unknown-linux-musl
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY src ./src
RUN cargo build --release --locked --jobs 2 --target x86_64-unknown-linux-musl

FROM nvidia/cuda:13.2.0-cudnn-devel-ubuntu24.04
ARG BUILD_JOBS=2
ARG BASALT_REVISION=a274a20c65775d1dfabe718a15dd9cac6f42e326
LABEL io.k-lani.basalt.revision="${BASALT_REVISION}"
RUN apt-get update && apt-get install -y --no-install-recommends cmake ninja-build g++ git python3 python3-venv ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/basalt
COPY --from=basalt_source /CMakeLists.txt ./
COPY --from=basalt_source /cmake ./cmake
COPY --from=basalt_source /src ./src
COPY --from=basalt_source /include ./include
COPY --from=basalt_source /data ./data
COPY --from=basalt_source /tests ./tests
COPY --from=basalt_source /serve ./serve
COPY --from=basalt_source /tools ./tools
COPY --from=basalt_source /requirements.txt /LICENSE /THIRD_PARTY_LICENSES ./
COPY integrations/basalt/privacy.patch /tmp/link-privacy.patch
RUN if git apply --check /tmp/link-privacy.patch; then git apply /tmp/link-privacy.patch; \
    else git apply --reverse --check /tmp/link-privacy.patch; fi
RUN python3 -m venv .venv && .venv/bin/pip install --no-cache-dir -r requirements.txt \
    && cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build --target basalt -j "$BUILD_JOBS" \
    && rm -rf build
RUN if getent passwd 1000 >/dev/null; then \
      usermod --login link --home /home/link "$(getent passwd 1000 | cut -d: -f1)"; \
    else useradd --create-home --uid 1000 link; fi \
    && mkdir -p /run/link /home/link/.cache \
    && chown -R 1000:1000 /run/link /home/link
COPY --from=link /source/target/x86_64-unknown-linux-musl/release/k-lani-link /usr/local/bin/k-lani-link
COPY --from=llama /opt/llama /opt/llama
ENV PYTHONDONTWRITEBYTECODE=1 CUDA_CACHE_DISABLE=1
COPY integrations/basalt/numeric-metrics.patch /tmp/link-numeric-metrics.patch
RUN git apply --check /tmp/link-numeric-metrics.patch \
    && git apply /tmp/link-numeric-metrics.patch \
    && .venv/bin/python -B -m unittest serve.tests.test_numeric_metrics serve.tests.test_privacy -q
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md /usr/share/doc/k-lani-link/
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/k-lani-link", "backend", "start"]
