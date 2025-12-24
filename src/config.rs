//! Configuration for the proxy.

use serde::{Deserialize, Serialize};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Configuration validation errors.
#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum ConfigError {
    /// Invalid listen address format.
    #[error("invalid listen address '{addr}': {reason}")]
    InvalidListenAddr { addr: String, reason: String },

    /// Invalid metrics address format.
    #[error("invalid metrics address '{addr}': {reason}")]
    InvalidMetricsAddr { addr: String, reason: String },

    /// Invalid upstream address format.
    #[error("invalid upstream address '{addr}': {reason}")]
    InvalidUpstreamAddr { addr: String, reason: String },

    /// No upstream addresses configured.
    #[error("at least one upstream address is required")]
    NoUpstreamAddrs,

    /// Invalid timeout value.
    #[error("invalid timeout value: {reason}")]
    InvalidTimeout { reason: String },

    /// Duplicate listen and metrics addresses.
    #[error("listen address and metrics address cannot be the same: {addr}")]
    DuplicateAddrs { addr: String },
}

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
/// * `PROXY_MAX_CONNECTIONS` - Maximum concurrent connections (default: 10000)
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

    /// Maximum concurrent connections.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

fn default_max_connections() -> usize {
    10_000
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:3000".to_string(),
            upstream_addrs: vec!["http://127.0.0.1:8080".to_string()],
            metrics_addr: "127.0.0.1:9090".to_string(),
            request_timeout: Duration::from_secs(30),
            max_connections: default_max_connections(),
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

        let max_connections = env::var("PROXY_MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or_else(default_max_connections);

        Self {
            listen_addr,
            upstream_addrs,
            metrics_addr,
            request_timeout: Duration::from_millis(request_timeout_ms),
            max_connections,
        }
    }

    /// Loads configuration from environment variables and validates it.
    ///
    /// Returns an error if the configuration is invalid.
    #[allow(dead_code)]
    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }

    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Listen address is not a valid socket address
    /// - Metrics address is not a valid socket address
    /// - No upstream addresses are configured
    /// - Upstream addresses have invalid URL format
    /// - Listen and metrics addresses are the same
    /// - Timeout is zero or too large
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate listen address
        self.listen_addr
            .parse::<SocketAddr>()
            .map_err(|e| ConfigError::InvalidListenAddr {
                addr: self.listen_addr.clone(),
                reason: e.to_string(),
            })?;

        // Validate metrics address
        self.metrics_addr
            .parse::<SocketAddr>()
            .map_err(|e| ConfigError::InvalidMetricsAddr {
                addr: self.metrics_addr.clone(),
                reason: e.to_string(),
            })?;

        // Check for duplicate addresses
        if self.listen_addr == self.metrics_addr {
            return Err(ConfigError::DuplicateAddrs {
                addr: self.listen_addr.clone(),
            });
        }

        // Validate upstream addresses
        if self.upstream_addrs.is_empty() {
            return Err(ConfigError::NoUpstreamAddrs);
        }

        for addr in &self.upstream_addrs {
            // Basic URL validation
            if !addr.starts_with("http://") && !addr.starts_with("https://") {
                return Err(ConfigError::InvalidUpstreamAddr {
                    addr: addr.clone(),
                    reason: "must start with http:// or https://".to_string(),
                });
            }

            // Try to parse the URL
            if url::Url::parse(addr).is_err() {
                return Err(ConfigError::InvalidUpstreamAddr {
                    addr: addr.clone(),
                    reason: "invalid URL format".to_string(),
                });
            }
        }

        // Validate timeout
        if self.request_timeout.is_zero() {
            return Err(ConfigError::InvalidTimeout {
                reason: "timeout must be greater than zero".to_string(),
            });
        }

        if self.request_timeout > Duration::from_secs(3600) {
            return Err(ConfigError::InvalidTimeout {
                reason: "timeout must not exceed 1 hour".to_string(),
            });
        }

        Ok(())
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
        assert_eq!(config.max_connections, 10_000);
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

    #[test]
    fn test_validate_valid_config() {
        let config = ProxyConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_listen_addr() {
        let config = ProxyConfig {
            listen_addr: "invalid".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidListenAddr { .. }
        ));
    }

    #[test]
    fn test_validate_invalid_metrics_addr() {
        let config = ProxyConfig {
            metrics_addr: "invalid".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidMetricsAddr { .. }
        ));
    }

    #[test]
    fn test_validate_duplicate_addrs() {
        let config = ProxyConfig {
            listen_addr: "127.0.0.1:3000".to_string(),
            metrics_addr: "127.0.0.1:3000".to_string(),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::DuplicateAddrs { .. }
        ));
    }

    #[test]
    fn test_validate_no_upstream() {
        let config = ProxyConfig {
            upstream_addrs: vec![],
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::NoUpstreamAddrs));
    }

    #[test]
    fn test_validate_invalid_upstream() {
        let config = ProxyConfig {
            upstream_addrs: vec!["not-a-url".to_string()],
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidUpstreamAddr { .. }
        ));
    }

    #[test]
    fn test_validate_zero_timeout() {
        let config = ProxyConfig {
            request_timeout: Duration::ZERO,
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidTimeout { .. }
        ));
    }

    #[test]
    fn test_validate_excessive_timeout() {
        let config = ProxyConfig {
            request_timeout: Duration::from_secs(7200),
            ..Default::default()
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ConfigError::InvalidTimeout { .. }
        ));
    }
}
