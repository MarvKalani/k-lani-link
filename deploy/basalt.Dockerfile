# llama.cpp for GGUF models in the managed catalog (deploy/managed). Pinned by digest; it
# brings its own CUDA 12 libraries, which only its process loads. server-cuda-b12182.
FROM ghcr.io/ggml-org/llama.cpp@sha256:77e84a5d6d885c07afbda0ae2fe48f4943f5181c8c8fe3b2e4231e8a1498bb24 AS llama
RUN mkdir -p /opt/llama/lib && cp -a /app/. /opt/llama/ \
    && cp -L /usr/local/cuda/lib64/libcudart.so.12 /usr/local/cuda/lib64/libcublas.so.12 \
       /usr/local/cuda/lib64/libcublasLt.so.12 /lib/x86_64-linux-gnu/libnccl.so.2 /opt/llama/lib/ \
    && rm -f /opt/llama/llama /opt/llama/*bench* /opt/llama/*perplexity* /opt/llama/*quantize*

# The supervisor, entrypoint and health probe: k-lani-link itself, statically linked so it
# runs on the CUDA image's own libc. No interpreter of ours ships in the image.
FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS link
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools cmake g++ pkg-config \
    && rm -rf /var/lib/apt/lists/* && rustup target add x86_64-unknown-linux-musl
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY src ./src
RUN cargo build --release --locked --jobs 2 --target x86_64-unknown-linux-musl

FROM ubuntu:24.04@sha256:2198e4c65076e9cb0b3d695219df667c0e3b0072da64ca3c2648aca17dcecaf5 AS serving
RUN apt-get update && apt-get install -y --no-install-recommends git python3-venv ca-certificates \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/basalt
COPY --from=basalt_source /CMakeLists.txt ./
COPY --from=basalt_source /serve ./serve
COPY --from=basalt_source /tools ./tools
COPY --from=basalt_source /requirements.txt ./
COPY integrations/basalt/privacy.patch /tmp/link-privacy.patch
COPY integrations/basalt/numeric-metrics.patch /tmp/link-numeric-metrics.patch
RUN for patch in /tmp/link-privacy.patch /tmp/link-numeric-metrics.patch; do \
      git apply "$patch" || { echo "Cannot apply $patch to the selected Basalt source" >&2; exit 1; }; \
    done
RUN python3 -m venv .venv && .venv/bin/pip install --no-cache-dir -r requirements.txt

FROM nvidia/cuda:13.5.0-cudnn-devel-ubuntu24.04@sha256:f24a2953ce729567af330c06d850a4f61f3a58df6901f9189d3319ea8881c107 AS build
ARG BUILD_JOBS=2
RUN apt-get update && apt-get install -y --no-install-recommends cmake ninja-build g++ git \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/basalt
COPY --from=basalt_source /CMakeLists.txt ./
COPY --from=basalt_source /cmake ./cmake
COPY --from=basalt_source /src ./src
COPY --from=basalt_source /include ./include
COPY --from=basalt_source /data ./data
COPY --from=basalt_source /tests ./tests
COPY --from=basalt_source /tools ./tools
# Depend on the serving stage while keeping the native input independent of the Python patch.
RUN --mount=type=bind,from=serving,source=/opt/basalt/CMakeLists.txt,target=/tmp/basalt-source.cmake \
    cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
    && cmake --build build --target basalt -j "$BUILD_JOBS" \
    && rm -rf build

# Basalt links cudart and cuBLAS. The base carries cudart; copy the two cuBLAS
# runtime libraries from the build toolkit. llama.cpp keeps its own CUDA 12 libraries.
FROM nvidia/cuda:13.5.0-base-ubuntu24.04@sha256:8c30b31b13a97ee353c21e666b227dea44f4331d38a221f257de70fc04b437d0 AS runtime
ARG BASALT_REVISION=2729e2288f59e01836ddd7fcc630810ada98995a
LABEL io.k-lani.basalt.revision="${BASALT_REVISION}"
RUN apt-get update && apt-get install -y --no-install-recommends python3 libstdc++6 libgomp1 ca-certificates \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/basalt
COPY --from=build /opt/basalt/bin ./bin
COPY --from=serving /opt/basalt/serve ./serve
COPY --from=serving /opt/basalt/tools ./tools
COPY --from=build /opt/basalt/data ./data
COPY --from=serving /opt/basalt/.venv ./.venv
COPY --from=serving /opt/basalt/requirements.txt ./
COPY --from=basalt_source /LICENSE /THIRD_PARTY_LICENSES ./
COPY --from=build /usr/local/cuda/lib64/libcublas.so.13.* /usr/local/cuda/lib64/libcublasLt.so.13.* /usr/local/cuda/lib64/
RUN ldconfig
RUN if getent passwd 1000 >/dev/null; then \
      usermod --login link --home /home/link "$(getent passwd 1000 | cut -d: -f1)"; \
    else useradd --create-home --uid 1000 link; fi \
    && mkdir -p /run/link /home/link/.cache \
    && chown -R 1000:1000 /run/link /home/link
COPY --from=link /source/target/x86_64-unknown-linux-musl/release/k-lani-link /usr/local/bin/k-lani-link
COPY --from=llama /opt/llama /opt/llama
ENV PYTHONDONTWRITEBYTECODE=1 CUDA_CACHE_DISABLE=1
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md /usr/share/doc/k-lani-link/
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/k-lani-link", "backend", "start"]

# Gate the final image on the same server suite used for the source checkout.
# This stage has the compiled engine for model-free flag and server tests.
FROM build AS tests
COPY --from=serving /opt/basalt/serve ./serve
COPY --from=serving /opt/basalt/.venv ./.venv
COPY --from=serving /opt/basalt/requirements.txt ./
COPY --from=basalt_source /docs ./docs
RUN apt-get update && apt-get install -y --no-install-recommends python3 openssl \
    && apt-get clean && rm -rf /var/lib/apt/lists/*
RUN --network=none .venv/bin/python -B -m unittest discover -s serve/tests -q >/tmp/serve-tests.log 2>&1 \
    && tail -n 4 /tmp/serve-tests.log \
    && touch /tmp/basalt-tests-passed \
    || { tail -n 200 /tmp/serve-tests.log; exit 1; }

FROM runtime
COPY --from=tests /tmp/basalt-tests-passed /usr/share/doc/k-lani-link/basalt-tests-passed
