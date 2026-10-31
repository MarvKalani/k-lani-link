# Instrumentation image for synthetic tests; not a production relay image.
FROM python@sha256:f77ac9e44ae96ef2c90b8053ea08c31f8be030f824196b0ae4db6d462c84e51f
RUN apt-get update && apt-get install --no-install-recommends -y nginx iproute2 util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY link-mailbox-bench /bench/link-mailbox-bench
COPY nginx.conf entrypoint.sh /bench/
ENTRYPOINT ["/bin/sh", "/bench/entrypoint.sh"]
