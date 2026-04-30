# ── Build stage ────────────────────────────────────────────────────────────────
# edition 2024 requires Rust 1.85+
FROM rust:1.85-slim AS builder

WORKDIR /build

# Cache dependency compilation separately from application code.
# Copy manifests first; only re-run cargo fetch / compile when they change.
COPY Cargo.toml Cargo.lock ./

# Build a throw-away binary to warm the dependency cache
RUN mkdir -p src && \
    echo 'fn main() {}' > src/main.rs && \
    echo ''              > src/lib.rs  && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Build the real application
COPY src    ./src
COPY tests  ./tests
RUN touch src/main.rs src/lib.rs && cargo build --release

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

# ca-certificates is needed for reqwest (rustls) to verify upstream TLS certs
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/oproxy ./
COPY configs ./configs

# Directories created at runtime; declaring them makes intent explicit and
# allows volume mounts to overlay them cleanly.
RUN mkdir -p certs storage

# Declare volumes so the CA key/cert and rule storage survive container restarts.
# Mount these with named volumes (-v oproxy-certs:/app/certs) to persist across
# container replacements (docker rm + docker run).
VOLUME ["/app/certs", "/app/storage"]

# Default port — override with OPROXY_PORT or by editing configs/default.json
EXPOSE 8080

# OPROXY_CONFIG   — path to the JSON config file
# OPROXY_PORT     — port override (takes precedence over the config file)
# OPROXY_MITM_ENABLED — set to "true" to enable HTTPS interception
ENV OPROXY_CONFIG=/app/configs/default.yaml

CMD ["./oproxy"]
