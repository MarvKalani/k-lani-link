# Test instrumentation only. No production model or configuration is included.
FROM python:3.12-slim@sha256:f77ac9e44ae96ef2c90b8053ea08c31f8be030f824196b0ae4db6d462c84e51f
RUN apt-get update && apt-get install --no-install-recommends -y iptables \
    && rm -rf /var/lib/apt/lists/* && mkdir -p /lab && chmod 0755 /lab
COPY --chmod=0755 k-lani-link /usr/local/bin/k-lani-link
COPY --chmod=0644 endpoint.py /lab/endpoint.py
ENV PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1
ENTRYPOINT ["python3", "/lab/endpoint.py"]
