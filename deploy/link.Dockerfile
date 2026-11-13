FROM rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7 AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake g++ pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY src ./src
RUN cargo build --release --locked --jobs 2

# Same Debian release as the build stage, so the binary finds the libc it was linked against.
FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2
RUN useradd --create-home --uid 1000 link \
    && install -d -m 0700 -o 1000 -g 1000 /var/lib/link
COPY --from=build /source/target/release/k-lani-link /usr/local/bin/k-lani-link
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md THIRD_PARTY.md /usr/share/doc/k-lani-link/
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/k-lani-link"]
CMD ["host", "--config", "/run/secrets/host.toml"]
