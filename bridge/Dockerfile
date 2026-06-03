# Multi-stage build for the off-chain bridge stack (validator / keeper / sig-store).
# One image carries all three binaries; the compose service picks which to run.

FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p validator -p keeper -p sig-store

FROM debian:bookworm-slim
# ca-certificates + libssl3 cover reqwest's TLS stack (HTTPS RPCs / sig-store).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/validator  /usr/local/bin/validator
COPY --from=builder /build/target/release/keeper      /usr/local/bin/keeper
COPY --from=builder /build/target/release/sig-store   /usr/local/bin/sig-store
ENV RUST_LOG=info
# Overridden per-service in docker-compose.yml.
CMD ["sig-store"]
