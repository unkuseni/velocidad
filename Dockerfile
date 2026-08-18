# syntax=docker/dockerfile:1
# ── build ───────────────────────────────────────────────────────────────────
FROM rust:1.97-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked

# ── runtime ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update     && apt-get install -y --no-install-recommends ca-certificates curl tzdata     && rm -rf /var/lib/apt/lists/*     && useradd -r -u 10001 appuser     && mkdir -p /data && chown -R appuser:appuser /data
COPY --from=builder /app/target/release/velocidad /usr/local/bin/velocidad
USER appuser
# Data directory (SQLite db + master key) — mount a volume here in production.
WORKDIR /data
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s     CMD curl -fsS http://127.0.0.1:8080/health >/dev/null || exit 1
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/velocidad"]
