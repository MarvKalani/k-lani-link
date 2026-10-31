FROM nvidia/cuda:13.2.0-cudnn-devel-ubuntu24.04
ARG BUILD_JOBS=2
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
COPY deploy/basalt-entrypoint.py /opt/link-entrypoint.py
COPY deploy/healthcheck.py /opt/link-healthcheck.py
COPY deploy/managed/manager.py /opt/link-manager.py
ENV PYTHONDONTWRITEBYTECODE=1 CUDA_CACHE_DISABLE=1
COPY integrations/basalt/numeric-metrics.patch /tmp/link-numeric-metrics.patch
RUN git apply --check /tmp/link-numeric-metrics.patch \
    && git apply /tmp/link-numeric-metrics.patch \
    && .venv/bin/python -B -m unittest serve.tests.test_numeric_metrics serve.tests.test_privacy -q
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md /usr/share/doc/k-lani-link/
USER 1000:1000
ENTRYPOINT ["/opt/basalt/.venv/bin/python", "-B", "/opt/link-entrypoint.py"]
