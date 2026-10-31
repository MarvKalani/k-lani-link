# Build the static binary with the commands in README.md before building this image.
# No shell, init, package manager, proxy or writable filesystem is needed by the relay.
FROM scratch
COPY target/x86_64-unknown-linux-musl/release/k-lani-link /k-lani-link
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md THIRD_PARTY.md /licenses/
USER 1000:1000
ENTRYPOINT ["/k-lani-link"]
CMD ["https-relay", "--config", "/config/relay.toml"]
