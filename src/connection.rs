//! Connection management and limiting.
//!
//! Provides connection tracking and limits to prevent resource exhaustion.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

/// Configuration for connection limits.
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// Maximum number of concurrent connections.
    pub max_connections: usize,
    /// Maximum number of connections per client IP.
    pub max_connections_per_ip: usize,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            max_connections: 10_000,
            max_connections_per_ip: 100,
        }
    }
}

impl ConnectionConfig {
    /// Creates a new connection configuration.
    pub fn new(max_connections: usize) -> Self {
        Self {
            max_connections,
            ..Default::default()
        }
    }

    /// Sets the maximum connections per IP.
    pub fn with_max_per_ip(mut self, max: usize) -> Self {
        self.max_connections_per_ip = max;
        self
    }
}

/// Connection limiter to prevent resource exhaustion.
///
/// Uses a semaphore to limit the total number of concurrent connections.
#[derive(Debug)]
pub struct ConnectionLimiter {
    /// Semaphore for limiting total connections.
    semaphore: Arc<Semaphore>,
    /// Current number of active connections.
    active_connections: AtomicUsize,
    /// Total connections accepted.
    total_accepted: AtomicUsize,
    /// Total connections rejected due to limits.
    total_rejected: AtomicUsize,
    /// Configuration.
    config: ConnectionConfig,
}

impl ConnectionLimiter {
    /// Creates a new connection limiter with the given configuration.
    pub fn new(config: ConnectionConfig) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_connections)),
            active_connections: AtomicUsize::new(0),
            total_accepted: AtomicUsize::new(0),
            total_rejected: AtomicUsize::new(0),
            config,
        }
    }

    /// Creates a connection limiter with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ConnectionConfig::default())
    }

    /// Attempts to acquire a connection permit.
    ///
    /// Returns `Some(ConnectionGuard)` if a connection is allowed,
    /// or `None` if the limit has been reached.
    pub fn try_acquire(&self) -> Option<ConnectionGuard<'_>> {
        match self.semaphore.clone().try_acquire_owned() {
            Ok(permit) => {
                self.active_connections.fetch_add(1, Ordering::Relaxed);
                self.total_accepted.fetch_add(1, Ordering::Relaxed);
                debug!(
                    active = self.active_connections.load(Ordering::Relaxed),
                    "connection acquired"
                );
                Some(ConnectionGuard {
                    _permit: permit,
                    active_counter: &self.active_connections,
                })
            }
            Err(_) => {
                self.total_rejected.fetch_add(1, Ordering::Relaxed);
                warn!(
                    max = self.config.max_connections,
                    "connection limit reached, rejecting"
                );
                None
            }
        }
    }

    /// Returns the number of active connections.
    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Returns the total number of accepted connections.
    pub fn total_accepted(&self) -> usize {
        self.total_accepted.load(Ordering::Relaxed)
    }

    /// Returns the total number of rejected connections.
    pub fn total_rejected(&self) -> usize {
        self.total_rejected.load(Ordering::Relaxed)
    }

    /// Returns the maximum allowed connections.
    pub fn max_connections(&self) -> usize {
        self.config.max_connections
    }

    /// Returns connection statistics.
    pub fn stats(&self) -> ConnectionStats {
        ConnectionStats {
            active: self.active_connections.load(Ordering::Relaxed),
            total_accepted: self.total_accepted.load(Ordering::Relaxed),
            total_rejected: self.total_rejected.load(Ordering::Relaxed),
            max_connections: self.config.max_connections,
        }
    }
}

/// Guard that releases a connection permit when dropped.
pub struct ConnectionGuard<'a> {
    _permit: tokio::sync::OwnedSemaphorePermit,
    active_counter: &'a AtomicUsize,
}

impl<'a> Drop for ConnectionGuard<'a> {
    fn drop(&mut self) {
        self.active_counter.fetch_sub(1, Ordering::Relaxed);
        debug!(
            active = self.active_counter.load(Ordering::Relaxed),
            "connection released"
        );
    }
}

/// Statistics about connection usage.
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    /// Current number of active connections.
    pub active: usize,
    /// Total number of accepted connections.
    pub total_accepted: usize,
    /// Total number of rejected connections due to limits.
    pub total_rejected: usize,
    /// Maximum allowed connections.
    pub max_connections: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_limiter_basic() {
        let limiter = ConnectionLimiter::new(ConnectionConfig::new(2));

        let _guard1 = limiter.try_acquire();
        let _guard2 = limiter.try_acquire();
        assert!(_guard1.is_some());
        assert!(_guard2.is_some());
        assert_eq!(limiter.active_connections(), 2);

        // Should reject - at limit
        assert!(limiter.try_acquire().is_none());
        assert_eq!(limiter.total_rejected(), 1);
    }

    #[test]
    fn test_connection_guard_release() {
        let limiter = ConnectionLimiter::new(ConnectionConfig::new(1));

        {
            let _guard = limiter.try_acquire();
            assert_eq!(limiter.active_connections(), 1);
        }

        // Guard dropped, should be able to acquire again
        assert_eq!(limiter.active_connections(), 0);
        assert!(limiter.try_acquire().is_some());
    }

    #[test]
    fn test_connection_stats() {
        let limiter = ConnectionLimiter::new(ConnectionConfig::new(2));

        let _guard1 = limiter.try_acquire();
        let _guard2 = limiter.try_acquire();
        let _ = limiter.try_acquire(); // Rejected

        let stats = limiter.stats();
        assert_eq!(stats.active, 2);
        assert_eq!(stats.total_accepted, 2);
        assert_eq!(stats.total_rejected, 1);
        assert_eq!(stats.max_connections, 2);
    }
}
