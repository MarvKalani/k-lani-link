FROM rust:1.96.0-slim-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends cmake g++ pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY src ./src
RUN cargo build --release --locked --jobs 2

FROM python:3.12-slim
RUN useradd --create-home --uid 1000 link \
    && install -d -m 0700 -o 1000 -g 1000 /var/lib/link
COPY --from=build /source/target/release/k-lani-link /usr/local/bin/k-lani-link
COPY LICENSE LICENSING.md COMMERCIAL-LICENSE.md THIRD_PARTY.md /usr/share/doc/k-lani-link/
USER 1000:1000
ENTRYPOINT ["/usr/local/bin/k-lani-link"]
CMD ["host", "--config", "/run/secrets/host.toml"]
