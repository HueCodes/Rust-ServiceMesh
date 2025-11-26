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
    #[allow(dead_code)]
    AcceptConnection(#[source] io::Error),

    /// Failed to connect to upstream server.
    #[error("failed to connect to upstream {addr}: {source}")]
    #[allow(dead_code)]
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
}

/// Result type alias for proxy operations.
pub type Result<T> = std::result::Result<T, ProxyError>;
