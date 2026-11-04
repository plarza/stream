# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.99.1

FROM rust:${RUST_VERSION}-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked && cp target/release/stream-node /stream-node

FROM debian:trixie-slim AS runtime

LABEL org.opencontainers.image.title="stream-node" \
    org.opencontainers.image.description="The Stream node: your own music files, for Stream on every device" \
    org.opencontainers.image.source="https://git.aza.network/aza/stream"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --system --gid 10001 stream \
    && useradd --system --uid 10001 --gid stream --home-dir /data --shell /usr/sbin/nologin stream

COPY --from=build /stream-node /usr/local/bin/stream-node
ARG SOURCE_REV=unknown
RUN printf '%s\n' "$SOURCE_REV" > /.source-rev

ENV STREAM_DATA=/data
EXPOSE 8080
USER stream
ENTRYPOINT ["/usr/local/bin/stream-node"]
