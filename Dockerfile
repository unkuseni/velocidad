# syntax=docker/dockerfile:1
# ── build ───────────────────────────────────────────────────────────────────
FROM rust:1.97-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

# ── runtime ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 appuser
COPY --from=builder /app/target/release/velocidad /usr/local/bin/velocidad
USER appuser
# Data directory (SQLite db + master key) — mount a volume here in production.
WORKDIR /data
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/velocidad"]
