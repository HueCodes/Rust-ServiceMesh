//! Configuration for the proxy.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Proxy configuration loaded at startup.
///
/// Immutable after initialization and shared across tasks via `Arc`.
///
/// # Example
///
/// ```
/// use rust_servicemesh::config::ProxyConfig;
///
/// let config = ProxyConfig {
///     listen_addr: "127.0.0.1:3000".to_string(),
///     upstream_addrs: vec!["http://127.0.0.1:8080".to_string()],
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Address to listen on for incoming connections.
    pub listen_addr: String,

    /// List of upstream server addresses for load balancing.
    pub upstream_addrs: Vec<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:3000".to_string(),
            upstream_addrs: vec!["http://127.0.0.1:8080".to_string()],
        }
    }
}

impl ProxyConfig {
    /// Wraps the config in an `Arc` for shared ownership.
    pub fn into_arc(self) -> Arc<Vec<String>> {
        Arc::new(self.upstream_addrs)
    }
}
