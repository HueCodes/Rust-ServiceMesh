//! Configuration for the proxy.

use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use std::time::Duration;

/// Proxy configuration loaded at startup.
///
/// Immutable after initialization and shared across tasks via `Arc`.
/// Configuration can be loaded from environment variables or defaults.
///
/// # Environment Variables
///
/// * `PROXY_LISTEN_ADDR` - Address to listen on (default: "127.0.0.1:3000")
/// * `PROXY_UPSTREAM_ADDRS` - Comma-separated upstream addresses (default: "http://127.0.0.1:8080")
/// * `PROXY_METRICS_ADDR` - Metrics endpoint address (default: "127.0.0.1:9090")
/// * `PROXY_REQUEST_TIMEOUT_MS` - Request timeout in milliseconds (default: 30000)
///
/// # Example
///
/// ```
/// use rust_servicemesh::config::ProxyConfig;
///
/// let config = ProxyConfig::from_env();
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Address to listen on for incoming connections.
    pub listen_addr: String,

    /// List of upstream server addresses for load balancing.
    pub upstream_addrs: Vec<String>,

    /// Address to serve metrics on.
    pub metrics_addr: String,

    /// Request timeout duration.
    pub request_timeout: Duration,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:3000".to_string(),
            upstream_addrs: vec!["http://127.0.0.1:8080".to_string()],
            metrics_addr: "127.0.0.1:9090".to_string(),
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl ProxyConfig {
    /// Loads configuration from environment variables with fallback to defaults.
    ///
    /// # Example
    ///
    /// ```
    /// use rust_servicemesh::config::ProxyConfig;
    ///
    /// let config = ProxyConfig::from_env();
    /// assert!(!config.upstream_addrs.is_empty());
    /// ```
    pub fn from_env() -> Self {
        let listen_addr =
            env::var("PROXY_LISTEN_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());

        let upstream_addrs = env::var("PROXY_UPSTREAM_ADDRS")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let metrics_addr =
            env::var("PROXY_METRICS_ADDR").unwrap_or_else(|_| "127.0.0.1:9090".to_string());

        let request_timeout_ms = env::var("PROXY_REQUEST_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30000);

        Self {
            listen_addr,
            upstream_addrs,
            metrics_addr,
            request_timeout: Duration::from_millis(request_timeout_ms),
        }
    }

    /// Wraps the upstream addresses in an `Arc` for shared ownership.
    pub fn upstream_addrs_arc(&self) -> Arc<Vec<String>> {
        Arc::new(self.upstream_addrs.clone())
    }

    /// Returns the request timeout duration.
    #[allow(dead_code)]
    pub fn timeout(&self) -> Duration {
        self.request_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ProxyConfig::default();
        assert_eq!(config.listen_addr, "127.0.0.1:3000");
        assert_eq!(config.upstream_addrs.len(), 1);
        assert_eq!(config.metrics_addr, "127.0.0.1:9090");
    }

    #[test]
    fn test_from_env() {
        let config = ProxyConfig::from_env();
        assert!(!config.listen_addr.is_empty());
        assert!(!config.upstream_addrs.is_empty());
        assert!(!config.metrics_addr.is_empty());
    }

    #[test]
    fn test_timeout() {
        let config = ProxyConfig::default();
        assert_eq!(config.timeout(), Duration::from_secs(30));
    }
}
