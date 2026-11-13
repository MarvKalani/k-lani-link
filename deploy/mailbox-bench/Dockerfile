# Instrumentation image for synthetic tests; not a production relay image.
FROM python:3.15-slim@sha256:65a65fc7b30b06cb6df4930fd4650697cb23f4c2698ab81fc87e4b031bc36928
RUN apt-get update && apt-get install --no-install-recommends -y nginx iproute2 util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY link-mailbox-bench /bench/link-mailbox-bench
COPY nginx.conf entrypoint.sh /bench/
ENTRYPOINT ["/bin/sh", "/bench/entrypoint.sh"]
