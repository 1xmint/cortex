# ── Stage 1: Build ──────────────────────────────────────────────
FROM rust:1.88-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

# Cache dependency build: copy manifests first, create stub sources,
# build once, then copy real sources and rebuild.
COPY Cargo.toml Cargo.lock ./
COPY crates/core/Cargo.toml crates/core/Cargo.toml
COPY crates/engine/Cargo.toml crates/engine/Cargo.toml
COPY crates/api/Cargo.toml crates/api/Cargo.toml
COPY crates/worker/Cargo.toml crates/worker/Cargo.toml
COPY crates/soma/Cargo.toml crates/soma/Cargo.toml
COPY crates/soma-crypto/Cargo.toml crates/soma-crypto/Cargo.toml
COPY crates/soma-core/Cargo.toml crates/soma-core/Cargo.toml

# Create minimal stub files so cargo can resolve the workspace
RUN mkdir -p crates/core/src crates/engine/src crates/api/src crates/worker/src/bin \
             crates/soma/src crates/soma/src/bin crates/soma-crypto/src crates/soma-core/src \
    && echo "pub fn _stub() {}" > crates/core/src/lib.rs \
    && echo "pub fn _stub() {}" > crates/engine/src/lib.rs \
    && echo "pub fn _stub() {}" > crates/api/src/lib.rs \
    && echo "fn main() {}" > crates/api/src/main.rs \
    && echo "fn main() {}" > crates/worker/src/bin/worker.rs \
    && echo "pub fn _stub() {}" > crates/soma/src/lib.rs \
    && echo "fn main() {}" > crates/soma/src/bin/ceremony.rs \
    && echo "pub fn _stub() {}" > crates/soma-crypto/src/lib.rs \
    && echo "pub fn _stub() {}" > crates/soma-core/src/lib.rs

# Build deps only (this layer is cached until Cargo.toml/lock changes)
RUN cargo build --release -p cortex-api 2>/dev/null || true

# Copy real sources and rebuild
COPY crates/ crates/
RUN touch crates/core/src/lib.rs crates/engine/src/lib.rs \
         crates/api/src/lib.rs crates/api/src/main.rs \
         crates/worker/src/bin/worker.rs \
         crates/soma/src/lib.rs crates/soma-crypto/src/lib.rs crates/soma-core/src/lib.rs \
    && cargo build --release -p cortex-api -p cortex-worker

# ── Stage 2: Runtime ───────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates git curl nodejs npm \
    && rm -rf /var/lib/apt/lists/*

# Install replit-tools (includes Claude and Codex CLI tools)
RUN npm install -g replit-tools

# Non-root user
RUN groupadd --gid 1001 cortex \
    && useradd --uid 1001 --gid cortex --create-home cortex

# Persistent data directory for SQLite
RUN mkdir -p /data && chown cortex:cortex /data

COPY --from=builder --chown=cortex:cortex /src/target/release/cortex-server /usr/local/bin/cortex-server

# Copy initialization scripts
COPY --chown=cortex:cortex scripts/container-auth-init.sh /usr/local/bin/container-auth-init.sh
COPY --chown=cortex:cortex scripts/container-entrypoint.sh /usr/local/bin/container-entrypoint.sh
RUN chmod +x /usr/local/bin/container-auth-init.sh /usr/local/bin/container-entrypoint.sh

USER cortex
WORKDIR /home/cortex

ENV CORTEX_PORT=3001
ENV CORTEX_DB_PATH=/data/cortex.db
EXPOSE 3001

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:3001/api/health || exit 1

ENTRYPOINT ["/usr/local/bin/container-entrypoint.sh"]
