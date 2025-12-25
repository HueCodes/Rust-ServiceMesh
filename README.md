# Rust Service Mesh

[![CI](https://github.com/HueCodes/Rust-ServiceMesh/actions/workflows/ci.yml/badge.svg)](https://github.com/HueCodes/Rust-ServiceMesh/actions/workflows/ci.yml)

A high-performance service mesh data plane proxy built in Rust with Tokio, Hyper, Tower, and Rustls.

## Features

- **HTTP/1.1 and HTTP/2 Support**: Full protocol support with ALPN-based negotiation
- **TLS Termination**: Secure connections using Rustls with modern cipher suites
- **Load Balancing**: Round-robin, least connections, random, and weighted strategies
- **Circuit Breaker**: Hystrix-style fault tolerance with configurable thresholds
- **Rate Limiting**: Token bucket algorithm with per-client and global limits
- **L7 Routing**: Path, header, and method-based routing rules with regex support
- **Retry Logic**: Exponential backoff with jitter and configurable policies
- **Metrics**: Prometheus-compatible metrics export
- **Connection Pooling**: Efficient upstream connection management
- **Graceful Shutdown**: Clean shutdown with in-flight request completion

## Architecture

```
                                    +------------------+
                                    |   Upstream 1     |
                                    +------------------+
                                           ^
+----------+     +---------------+         |
|  Client  | --> |    Proxy      | --------+
+----------+     |               |         |
                 | +----------+  |         v
                 | | Listener |  |  +------------------+
                 | +----+-----+  |  |   Upstream 2     |
                 |      |        |  +------------------+
                 |      v        |         ^
                 | +----------+  |         |
                 | | Router   |--+---------+
                 | +----+-----+  |         |
                 |      |        |         v
                 |      v        |  +------------------+
                 | +----------+  |  |   Upstream N     |
                 | | Service  |  |  +------------------+
                 | +----------+  |
                 +---------------+
```

### Module Overview

| Module | Description |
|--------|-------------|
| `listener` | TCP/TLS listener with HTTP/1.1 and HTTP/2 protocol negotiation |
| `service` | Tower service implementation for request proxying |
| `router` | L7 routing with path, header, and method matching |
| `transport` | Connection pooling and load balancing |
| `circuit_breaker` | Fault tolerance with state machine |
| `ratelimit` | Token bucket rate limiting |
| `retry` | Exponential backoff retry logic |
| `protocol` | TLS and ALPN configuration |
| `metrics` | Prometheus metrics collection |
| `config` | Configuration management |
| `admin` | Health check and metrics endpoints |

## Quick Start

### Installation

```bash
# Clone the repository
git clone https://github.com/HueCodes/Rust-ServiceMesh.git
cd Rust-ServiceMesh

# Build in release mode
cargo build --release

# Run with default configuration
./target/release/proxy
```

### Docker

```bash
# Build the Docker image
docker build -t rust-servicemesh .

# Run the container
docker run -p 3000:3000 -p 9090:9090 rust-servicemesh
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PROXY_LISTEN_ADDR` | `127.0.0.1:3000` | Address to listen on |
| `PROXY_UPSTREAM_ADDRS` | `http://127.0.0.1:8080` | Comma-separated upstream addresses |
| `PROXY_METRICS_ADDR` | `127.0.0.1:9090` | Metrics endpoint address |
| `PROXY_REQUEST_TIMEOUT_MS` | `30000` | Request timeout in milliseconds |
| `RUST_LOG` | `info` | Log level (trace, debug, info, warn, error) |

### Example Usage

```bash
# Start the proxy
PROXY_UPSTREAM_ADDRS=http://localhost:8080,http://localhost:8081 \
PROXY_LISTEN_ADDR=0.0.0.0:3000 \
cargo run --release

# Test the proxy
curl http://localhost:3000/api/endpoint

# Check health
curl http://localhost:9090/health

# View metrics
curl http://localhost:9090/metrics
```

## Configuration Examples

### Basic HTTP Proxy

```rust
use rust_servicemesh::listener::Listener;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let upstream = Arc::new(vec!["http://localhost:8080".to_string()]);
    let timeout = Duration::from_secs(30);

    let listener = Listener::bind("127.0.0.1:3000", upstream, timeout).await?;

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
    listener.serve(shutdown_rx).await?;

    Ok(())
}
```

### HTTP/2 with TLS

```rust
use rust_servicemesh::listener::Listener;
use rust_servicemesh::protocol::{HttpProtocol, TlsConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let upstream = Arc::new(vec!["http://localhost:8080".to_string()]);
    let timeout = Duration::from_secs(30);

    let tls_config = TlsConfig::new("cert.pem", "key.pem")
        .with_protocol(HttpProtocol::Auto);

    let listener = Listener::bind_with_tls(
        "127.0.0.1:3443",
        upstream,
        timeout,
        tls_config,
    ).await?;

    let (_, shutdown_rx) = broadcast::channel(1);
    listener.serve(shutdown_rx).await?;

    Ok(())
}
```

### L7 Routing

```rust
use rust_servicemesh::router::{Router, Route, PathMatch, MethodMatch, HeaderMatch};

let mut router = Router::new();

// Exact path match
router.add_route(
    Route::new("users-api", PathMatch::exact("/api/users"), "users-cluster")
);

// Prefix match with method filter
router.add_route(
    Route::new("api", PathMatch::prefix("/api/"), "api-cluster")
        .with_method(MethodMatch::Get)
);

// Regex match with header requirement
router.add_route(
    Route::new("versioned", PathMatch::regex(r"^/v[0-9]+/.*"), "versioned-cluster")
        .with_header(HeaderMatch::present("authorization"))
);

// Path rewriting
router.add_route(
    Route::new("legacy", PathMatch::prefix("/old/"), "new-cluster")
        .with_rewrite("/new/")
);
```

### Circuit Breaker

```rust
use rust_servicemesh::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use std::time::Duration;

let config = CircuitBreakerConfig {
    failure_threshold: 5,
    timeout: Duration::from_secs(30),
    success_threshold: 2,
};

let cb = CircuitBreaker::new(config);

if cb.allow_request().await {
    match make_request().await {
        Ok(_) => cb.record_success().await,
        Err(_) => cb.record_failure().await,
    }
}
```

### Rate Limiting

```rust
use rust_servicemesh::ratelimit::{RateLimiter, RateLimitConfig};
use std::time::Duration;

let config = RateLimitConfig::new(100, 50)  // 100 req/s, burst of 50
    .with_per_client(true)
    .with_client_ttl(Duration::from_secs(300));

let limiter = RateLimiter::new(config);

match limiter.check(Some(client_ip)) {
    Ok(()) => { /* proceed with request */ }
    Err(info) => {
        // Return 429 with Retry-After header
        let retry_after = info.retry_after_secs();
    }
}
```

### Retry with Exponential Backoff

```rust
use rust_servicemesh::retry::{RetryConfig, RetryExecutor};
use std::time::Duration;

let config = RetryConfig::new()
    .with_max_retries(3)
    .with_base_delay(Duration::from_millis(100))
    .with_backoff_multiplier(2.0)
    .with_jitter(true);

let mut executor = RetryExecutor::new(config);

let result = executor.execute(|| async {
    make_request().await
}).await;
```

## Metrics

The proxy exposes Prometheus-compatible metrics at `/metrics`:

| Metric | Type | Description |
|--------|------|-------------|
| `http_requests_total` | Counter | Total HTTP requests by method, status, upstream |
| `http_request_duration_seconds` | Histogram | Request latency distribution |

Example Prometheus scrape config:

```yaml
scrape_configs:
  - job_name: 'rust-servicemesh'
    static_configs:
      - targets: ['localhost:9090']
```

## Benchmarks

Run benchmarks with:

```bash
cargo bench
```

Benchmark results (on Apple M1):

| Operation | Throughput |
|-----------|------------|
| Circuit breaker check | ~50M ops/sec |
| Rate limit check | ~20M ops/sec |
| Router exact match | ~30M ops/sec |
| Router prefix match | ~25M ops/sec |
| Router regex match | ~5M ops/sec |

## Development

### Prerequisites

- Rust 1.75 or later
- Cargo

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Build with all features
cargo build --features full
```

### Testing

```bash
# Run all tests
cargo test

# Run with logging
RUST_LOG=debug cargo test

# Run specific test
cargo test circuit_breaker

# Run benchmarks
cargo bench
```

### Code Quality

```bash
# Format code
cargo fmt

# Run clippy
cargo clippy --all-features -- -D warnings

# Generate docs
cargo doc --open
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.
