//! Rust Service Mesh - High-performance data plane proxy
//!
//! A service mesh proxy built with Rust, providing HTTP/1.1 and HTTP/2 proxying,
//! load balancing, circuit breaking, rate limiting, and observability.
//!
//! # Features
//!
//! - **HTTP/1.1 and HTTP/2 Support**: Full protocol support with ALPN negotiation
//! - **TLS Termination**: Secure connections with Rustls
//! - **Load Balancing**: Round-robin, least connections, random, and weighted strategies
//! - **Circuit Breaker**: Fault tolerance with configurable thresholds
//! - **Rate Limiting**: Token bucket algorithm with per-client and global limits
//! - **L7 Routing**: Path, header, and method-based routing rules
//! - **Retry Logic**: Exponential backoff with configurable retry policies
//! - **Metrics**: Prometheus-compatible metrics export
//!
//! # Quick Start
//!
//! ```no_run
//! use rust_servicemesh::listener::Listener;
//! use std::sync::Arc;
//! use std::time::Duration;
//! use tokio::sync::broadcast;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Configure upstream servers
//!     let upstream = Arc::new(vec!["http://localhost:8080".to_string()]);
//!     let timeout = Duration::from_secs(30);
//!
//!     // Create and start the proxy
//!     let listener = Listener::bind("127.0.0.1:3000", upstream, timeout).await?;
//!
//!     let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
//!     listener.serve(shutdown_rx).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Architecture
//!
//! The proxy is built using a modular architecture:
//!
//! - `listener`: TCP/TLS listener with protocol negotiation
//! - `service`: Tower service for request handling
//! - `router`: L7 routing with path/header matching
//! - `transport`: Connection pooling and load balancing
//! - `circuit_breaker`: Fault tolerance
//! - `ratelimit`: Request rate limiting
//! - `retry`: Retry logic with backoff
//! - `protocol`: HTTP/2 and TLS support
//! - `metrics`: Prometheus metrics
//! - `config`: Configuration management
//!
//! # Configuration
//!
//! The proxy can be configured via environment variables:
//!
//! - `PROXY_LISTEN_ADDR`: Address to listen on (default: "127.0.0.1:3000")
//! - `PROXY_UPSTREAM_ADDRS`: Comma-separated upstream addresses
//! - `PROXY_METRICS_ADDR`: Metrics endpoint address (default: "127.0.0.1:9090")
//! - `PROXY_REQUEST_TIMEOUT_MS`: Request timeout in milliseconds (default: 30000)

pub mod admin;
pub mod admin_listener;
pub mod circuit_breaker;
pub mod config;
pub mod error;
pub mod listener;
pub mod metrics;
pub mod protocol;
pub mod ratelimit;
pub mod retry;
pub mod router;
pub mod service;
pub mod transport;

// Re-export commonly used types
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, State as CircuitBreakerState};
pub use config::ProxyConfig;
pub use error::{ProxyError, Result};
pub use listener::Listener;
pub use protocol::{HttpProtocol, TlsConfig};
pub use ratelimit::{RateLimitConfig, RateLimiter};
pub use retry::{RetryConfig, RetryExecutor, RetryPolicy};
pub use router::{PathMatch, Route, Router};
pub use service::ProxyService;
pub use transport::{Endpoint, LoadBalancer, PoolConfig, Transport};
