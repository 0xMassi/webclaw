# webclaw — Multi-stage Docker build
# Produces 3 binaries:
#   webclaw         — CLI (single-shot extraction, crawl, MCP-less use)
#   webclaw-mcp     — MCP server (stdio, for AI agents)
#   webclaw-server  — minimal REST API for self-hosting (OSS, stateless)
#
# NOTE: this is NOT the hosted API at api.webclaw.io — the cloud service
# adds anti-bot bypass, JS rendering, multi-tenant auth and async jobs
# that are intentionally not open-source. See docs/self-hosting.

# ---------------------------------------------------------------------------
# Stage 1: Build all binaries in release mode
# ---------------------------------------------------------------------------
FROM rust:1.93-bookworm AS builder

# Build dependencies: cmake + clang for BoringSSL (wreq), pkg-config for linking
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    cmake \
    clang \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy manifests + lock first for better layer caching.
# If only source changes, cargo doesn't re-download deps.
COPY Cargo.toml Cargo.lock ./
COPY crates/webclaw-core/Cargo.toml crates/webclaw-core/Cargo.toml
COPY crates/webclaw-fetch/Cargo.toml crates/webclaw-fetch/Cargo.toml
COPY crates/webclaw-llm/Cargo.toml crates/webclaw-llm/Cargo.toml
COPY crates/webclaw-pdf/Cargo.toml crates/webclaw-pdf/Cargo.toml
COPY crates/webclaw-mcp/Cargo.toml crates/webclaw-mcp/Cargo.toml
COPY crates/webclaw-cli/Cargo.toml crates/webclaw-cli/Cargo.toml
COPY crates/webclaw-server/Cargo.toml crates/webclaw-server/Cargo.toml

# Copy .cargo config if present (optional build flags)
COPY .cargo .cargo

# Create dummy source files so cargo can resolve deps and cache them.
RUN mkdir -p crates/webclaw-core/src && echo "" > crates/webclaw-core/src/lib.rs \
    && mkdir -p crates/webclaw-fetch/src && echo "" > crates/webclaw-fetch/src/lib.rs \
    && mkdir -p crates/webclaw-llm/src && echo "" > crates/webclaw-llm/src/lib.rs \
    && mkdir -p crates/webclaw-pdf/src && echo "" > crates/webclaw-pdf/src/lib.rs \
    && mkdir -p crates/webclaw-mcp/src && echo "fn main() {}" > crates/webclaw-mcp/src/main.rs \
    && mkdir -p crates/webclaw-cli/src && echo "fn main() {}" > crates/webclaw-cli/src/main.rs \
    && mkdir -p crates/webclaw-server/src && echo "fn main() {}" > crates/webclaw-server/src/main.rs

# Pre-build dependencies (this layer is cached until Cargo.toml/lock changes)
RUN cargo build --release 2>/dev/null || true

# Now copy real source and rebuild. Only the final binaries recompile.
COPY crates crates
RUN touch crates/*/src/*.rs \
    && cargo build --release

# ---------------------------------------------------------------------------
# Stage 2: Minimal runtime image
# ---------------------------------------------------------------------------
FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy all three binaries
COPY --from=builder /build/target/release/webclaw /usr/local/bin/webclaw
COPY --from=builder /build/target/release/webclaw-mcp /usr/local/bin/webclaw-mcp
COPY --from=builder /build/target/release/webclaw-server /usr/local/bin/webclaw-server

# Default port the REST API listens on when you run `webclaw-server` inside
# the container. Override with -e WEBCLAW_PORT=... or --port. Published only
# as documentation; callers still need `-p 3000:3000` on `docker run`.
EXPOSE 3000

# Container default: bind all interfaces so `-p 3000:3000` works. Public
# binding requires WEBCLAW_API_KEY; the binary refuses open-auth 0.0.0.0
# unless WEBCLAW_ALLOW_OPEN_PUBLIC=1 is set explicitly for local testing.
ENV WEBCLAW_HOST=0.0.0.0

# Entrypoint shim: forwards webclaw args/URL to the binary, but exec's other
# commands directly so this image can be used as a FROM base with custom CMD.
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["webclaw", "--help"]
