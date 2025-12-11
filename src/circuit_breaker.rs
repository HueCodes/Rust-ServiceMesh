//! Circuit breaker implementation for fault tolerance.
//!
//! Implements a Hystrix-style circuit breaker with three states:
//! - **Closed**: Normal operation, requests flow through
//! - **Open**: Too many failures, reject requests immediately
//! - **HalfOpen**: Recovery mode, allow limited requests to test if service recovered

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Circuit is closed, requests flow normally
    Closed,
    /// Circuit is open, requests are rejected
    Open,
    /// Circuit is half-open, testing if service recovered
    HalfOpen,
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of failures before opening the circuit
    pub failure_threshold: u64,
    /// Duration to wait before transitioning from Open to HalfOpen
    pub timeout: Duration,
    /// Number of successful requests in HalfOpen before closing
    pub success_threshold: u64,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            timeout: Duration::from_secs(30),
            success_threshold: 2,
        }
    }
}

/// Circuit breaker for preventing cascading failures.
///
/// # Example
///
/// ```
/// use rust_servicemesh::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
///
/// #[tokio::main]
/// async fn main() {
///     let config = CircuitBreakerConfig::default();
///     let cb = CircuitBreaker::new(config);
///
///     if cb.allow_request().await {
///         // Make request
///         match make_request().await {
///             Ok(_) => cb.record_success().await,
///             Err(_) => cb.record_failure().await,
///         }
///     }
/// }
///
/// async fn make_request() -> Result<(), ()> {
///     Ok(())
/// }
/// ```
#[derive(Debug)]
pub struct CircuitBreaker {
    state: Arc<RwLock<State>>,
    failure_count: Arc<AtomicU64>,
    success_count: Arc<AtomicU64>,
    last_failure_time: Arc<RwLock<Option<Instant>>>,
    config: CircuitBreakerConfig,
    total_requests: Arc<AtomicUsize>,
    total_failures: Arc<AtomicUsize>,
}

impl CircuitBreaker {
    /// Creates a new circuit breaker with the given configuration.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(State::Closed)),
            failure_count: Arc::new(AtomicU64::new(0)),
            success_count: Arc::new(AtomicU64::new(0)),
            last_failure_time: Arc::new(RwLock::new(None)),
            config,
            total_requests: Arc::new(AtomicUsize::new(0)),
            total_failures: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Checks if a request should be allowed through.
    ///
    /// Returns `true` if the request should proceed, `false` if it should be rejected.
    pub async fn allow_request(&self) -> bool {
        self.total_requests.fetch_add(1, Ordering::Relaxed);

        let state = *self.state.read().await;

        match state {
            State::Closed => true,
            State::Open => {
                // Check if timeout has elapsed
                let last_failure = self.last_failure_time.read().await;
                if let Some(last_time) = *last_failure {
                    if last_time.elapsed() >= self.config.timeout {
                        drop(last_failure);
                        // Transition to HalfOpen
                        *self.state.write().await = State::HalfOpen;
                        self.success_count.store(0, Ordering::Relaxed);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            State::HalfOpen => true,
        }
    }

    /// Records a successful request.
    pub async fn record_success(&self) {
        let state = *self.state.read().await;

        match state {
            State::HalfOpen => {
                let successes = self.success_count.fetch_add(1, Ordering::Relaxed) + 1;
                if successes >= self.config.success_threshold {
                    // Transition to Closed
                    *self.state.write().await = State::Closed;
                    self.failure_count.store(0, Ordering::Relaxed);
                    self.success_count.store(0, Ordering::Relaxed);
                }
            }
            State::Closed => {
                // Reset failure count on success
                self.failure_count.store(0, Ordering::Relaxed);
            }
            State::Open => {}
        }
    }

    /// Records a failed request.
    pub async fn record_failure(&self) {
        self.total_failures.fetch_add(1, Ordering::Relaxed);

        let state = *self.state.read().await;

        match state {
            State::Closed => {
                let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                if failures >= self.config.failure_threshold {
                    // Transition to Open
                    *self.state.write().await = State::Open;
                    *self.last_failure_time.write().await = Some(Instant::now());
                }
            }
            State::HalfOpen => {
                // Immediately reopen on failure
                *self.state.write().await = State::Open;
                *self.last_failure_time.write().await = Some(Instant::now());
                self.failure_count.store(0, Ordering::Relaxed);
                self.success_count.store(0, Ordering::Relaxed);
            }
            State::Open => {
                *self.last_failure_time.write().await = Some(Instant::now());
            }
        }
    }

    /// Returns the current state of the circuit breaker.
    pub async fn state(&self) -> State {
        *self.state.read().await
    }

    /// Returns statistics about the circuit breaker.
    pub fn stats(&self) -> CircuitBreakerStats {
        CircuitBreakerStats {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            total_failures: self.total_failures.load(Ordering::Relaxed),
            current_failure_count: self.failure_count.load(Ordering::Relaxed),
            current_success_count: self.success_count.load(Ordering::Relaxed),
        }
    }

    /// Resets the circuit breaker to the closed state.
    #[allow(dead_code)]
    pub async fn reset(&self) {
        *self.state.write().await = State::Closed;
        self.failure_count.store(0, Ordering::Relaxed);
        self.success_count.store(0, Ordering::Relaxed);
        *self.last_failure_time.write().await = None;
    }
}

/// Statistics for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub total_requests: usize,
    pub total_failures: usize,
    pub current_failure_count: u64,
    pub current_success_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_circuit_breaker_closed_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 3,
            timeout: Duration::from_millis(100),
            success_threshold: 2,
        };
        let cb = CircuitBreaker::new(config);

        assert_eq!(cb.state().await, State::Closed);
        assert!(cb.allow_request().await);

        // Record failures
        cb.record_failure().await;
        cb.record_failure().await;
        cb.record_failure().await;

        assert_eq!(cb.state().await, State::Open);
        assert!(!cb.allow_request().await);
    }

    #[tokio::test]
    async fn test_circuit_breaker_open_to_halfopen() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            timeout: Duration::from_millis(50),
            success_threshold: 2,
        };
        let cb = CircuitBreaker::new(config);

        // Trigger open state
        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Open);

        // Wait for timeout
        sleep(Duration::from_millis(60)).await;

        // Should transition to HalfOpen
        assert!(cb.allow_request().await);
        assert_eq!(cb.state().await, State::HalfOpen);
    }

    #[tokio::test]
    async fn test_circuit_breaker_halfopen_to_closed() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            timeout: Duration::from_millis(50),
            success_threshold: 2,
        };
        let cb = CircuitBreaker::new(config);

        // Trigger open state
        cb.record_failure().await;
        cb.record_failure().await;

        // Wait for timeout and transition to HalfOpen
        sleep(Duration::from_millis(60)).await;
        assert!(cb.allow_request().await);

        // Record successes
        cb.record_success().await;
        cb.record_success().await;

        assert_eq!(cb.state().await, State::Closed);
    }

    #[tokio::test]
    async fn test_circuit_breaker_halfopen_to_open() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            timeout: Duration::from_millis(50),
            success_threshold: 2,
        };
        let cb = CircuitBreaker::new(config);

        // Trigger open state
        cb.record_failure().await;
        cb.record_failure().await;

        // Wait for timeout and transition to HalfOpen
        sleep(Duration::from_millis(60)).await;
        assert!(cb.allow_request().await);

        // Record failure in HalfOpen - should reopen
        cb.record_failure().await;
        assert_eq!(cb.state().await, State::Open);
    }

    #[tokio::test]
    async fn test_circuit_breaker_stats() {
        let config = CircuitBreakerConfig::default();
        let cb = CircuitBreaker::new(config);

        cb.allow_request().await;
        cb.allow_request().await;
        cb.record_failure().await;

        let stats = cb.stats();
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_failures, 1);
    }
}
