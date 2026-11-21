# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release -p adlink-cli

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 10001 adlink

WORKDIR /app
COPY --from=builder /build/target/release/adlink /usr/local/bin/adlink
COPY web-starter /app/web-starter
COPY config /app/config
RUN mkdir -p /app/data && chown -R adlink:adlink /app

USER adlink
EXPOSE 8787

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \
  CMD curl -fsS http://127.0.0.1:8787/api/v1/health || exit 1

ENTRYPOINT ["adlink"]
CMD ["serve", "--config", "/app/config/adlink.mock.toml", "--seed-mock"]
