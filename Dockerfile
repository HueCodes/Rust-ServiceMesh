# Build stage
FROM rust:1.75-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    echo "// dummy" > src/lib.rs

# Build dependencies only
RUN cargo build --release && rm -rf src

# Copy actual source code
COPY src ./src

# Build the application
RUN touch src/main.rs src/lib.rs && \
    cargo build --release --bin proxy

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /app/target/release/proxy /app/proxy

# Create non-root user
RUN useradd -r -s /bin/false proxy && \
    chown -R proxy:proxy /app

USER proxy

# Default environment variables
ENV PROXY_LISTEN_ADDR=0.0.0.0:3000
ENV PROXY_METRICS_ADDR=0.0.0.0:9090
ENV PROXY_UPSTREAM_ADDRS=http://localhost:8080
ENV PROXY_REQUEST_TIMEOUT_MS=30000
ENV RUST_LOG=info

# Expose ports
EXPOSE 3000 9090

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:9090/health || exit 1

# Run the proxy
ENTRYPOINT ["/app/proxy"]
