# ── UI build stage ─────────────────────────────────────────────────────────────
FROM node:22-bookworm-slim AS ui-builder

WORKDIR /ui
COPY src/design/package.json src/design/package-lock.json ./
RUN npm ci
COPY src/design ./
RUN npm run build

# ── Build stage ────────────────────────────────────────────────────────────────
# edition 2024 requires Rust 1.85+
FROM rust:1.95-slim AS builder

WORKDIR /build

# Cache dependency compilation separately from application code.
# Copy manifests first; only re-run cargo fetch / compile when they change.
COPY Cargo.toml Cargo.lock build.rs ./

# Build a throw-away binary to warm the dependency cache
RUN mkdir -p src && \
    echo 'fn main() {}' > src/main.rs && \
    echo ''              > src/lib.rs  && \
    cargo build --release 2>/dev/null || true && \
    rm -rf src

# Build the real application
COPY src    ./src
COPY tests  ./tests
COPY --from=ui-builder /ui/dist ./src/design/dist
RUN touch src/main.rs src/lib.rs && cargo build --release

# ── Runtime stage ──────────────────────────────────────────────────────────────
FROM debian:trixie-slim

# ca-certificates is needed for reqwest (rustls) to verify upstream TLS certs
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/oproxy ./
COPY configs ./configs

# Directories created at runtime; declaring them makes intent explicit and
# allows volume mounts to overlay them cleanly.
RUN groupadd --system oproxy && \
    useradd --system --gid oproxy --home-dir /app --no-create-home oproxy && \
    mkdir -p certs storage && \
    chown -R oproxy:oproxy /app

# Declare volumes so the CA key/cert, rule storage, hot config, and manually
# saved session files survive container restarts.
# Mount these with named volumes (-v oproxy-certs:/app/certs) to persist across
# container replacements (docker rm + docker run).
VOLUME ["/app/certs", "/app/storage"]

# Default ports - override with OPROXY_PORT / config if needed.
EXPOSE 8080 1080

# OPROXY_CONFIG   - path to the YAML config file
# OPROXY_PORT     - port override (takes precedence over the config file)
# OPROXY_MITM_ENABLED - set to "true" to enable HTTPS interception
# OPROXY_BIND_HOST - defaults to loopback; set 0.0.0.0 only with explicit port publishing
ENV OPROXY_CONFIG=/app/configs/default.yaml
ENV OPROXY_BIND_HOST=127.0.0.1

USER oproxy

CMD ["./oproxy"]
