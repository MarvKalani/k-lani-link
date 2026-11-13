# Test instrumentation only. No production model or configuration is included.
FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928
RUN apt-get update && apt-get install --no-install-recommends -y iptables \
    && rm -rf /var/lib/apt/lists/* && mkdir -p /lab && chmod 0755 /lab
COPY --chmod=0755 k-lani-link /usr/local/bin/k-lani-link
COPY --chmod=0644 endpoint.py /lab/endpoint.py
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
ENTRYPOINT ["python3", "/lab/endpoint.py"]
