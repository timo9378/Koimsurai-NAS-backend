# ============================================
# Koimsurai NAS Backend - Production Dockerfile
# Multi-stage build for Rust + FFmpeg
# ============================================

# Stage 1: Build
FROM rust:1.85-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy cargo files first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create dummy source to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src

# Copy actual source code
COPY src ./src
COPY migrations ./migrations

# Touch main.rs to ensure it rebuilds
RUN touch src/main.rs

# Build the application
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime

WORKDIR /app

# Install runtime dependencies including FFmpeg for transcoding
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    ffmpeg \
    procps \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -u 1001 -s /bin/bash appuser

# Copy the binary from builder
COPY --from=builder /app/target/release/Koimsurai_NAS /app/koimsurai-nas

# Create data directories
RUN mkdir -p /data/storage /data/db && chown -R appuser:appuser /data /app

USER appuser

EXPOSE 3000

ENV RUST_LOG=info
ENV DATABASE_URL=sqlite:///data/db/nas.db
ENV STORAGE_PATH=/data/storage

CMD ["/app/koimsurai-nas"]
