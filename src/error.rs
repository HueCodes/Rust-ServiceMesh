//! Error types for the service mesh proxy.

use std::io;
use thiserror::Error;

/// Errors that can occur during proxy operations.
#[derive(Error, Debug)]
pub enum ProxyError {
    /// Failed to bind to the listener address.
    #[error("failed to bind listener to {addr}: {source}")]
    ListenerBind { addr: String, source: io::Error },

    /// Failed to accept an incoming connection.
    #[error("failed to accept connection: {0}")]
    AcceptConnection(#[source] io::Error),

    /// Failed to connect to upstream server.
    #[error("failed to connect to upstream {addr}: {source}")]
    UpstreamConnect { addr: String, source: io::Error },

    /// HTTP protocol error.
    #[error("http error: {0}")]
    Http(#[from] hyper::Error),

    /// HTTP/2 error.
    #[error("http/2 error: {0}")]
    H2(#[from] hyper::http::Error),

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// No upstream servers available.
    #[error("no upstream servers available")]
    NoUpstream,

    /// Service unavailable.
    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    /// TLS configuration error.
    #[error("TLS configuration error: {message}")]
    TlsConfig { message: String },

    /// TLS handshake error.
    #[error("TLS handshake failed: {0}")]
    TlsHandshake(String),

    /// Protocol negotiation error.
    #[error("protocol negotiation failed: {0}")]
    ProtocolNegotiation(String),

    /// Rate limit exceeded.
    #[error("rate limit exceeded")]
    RateLimitExceeded,

    /// Circuit breaker is open.
    #[error("circuit breaker is open for upstream: {upstream}")]
    CircuitBreakerOpen { upstream: String },

    /// Request timeout.
    #[error("request timed out after {duration_ms}ms")]
    Timeout { duration_ms: u64 },

    /// Retry exhausted.
    #[error("all {attempts} retry attempts exhausted")]
    RetryExhausted { attempts: u32 },

    /// Invalid configuration.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// Route not found.
    #[error("no route found for path: {path}")]
    RouteNotFound { path: String },

    /// gRPC error.
    #[error("gRPC error: {message} (code: {code})")]
    Grpc { code: i32, message: String },
}

/// Result type alias for proxy operations.
pub type Result<T> = std::result::Result<T, ProxyError>;
