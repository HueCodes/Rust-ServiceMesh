//! Retry middleware with exponential backoff.
//!
//! Provides configurable retry logic for failed requests, with support for
//! exponential backoff, jitter, and customizable retry conditions.

use rand::Rng;
use std::time::Duration;
use tracing::{debug, warn};

/// Configuration for retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (excluding the initial request).
    pub max_retries: u32,
    /// Base delay between retries.
    pub base_delay: Duration,
    /// Maximum delay between retries.
    pub max_delay: Duration,
    /// Multiplier for exponential backoff.
    pub backoff_multiplier: f64,
    /// Whether to add jitter to delays.
    pub use_jitter: bool,
    /// HTTP status codes that should trigger a retry.
    pub retryable_status_codes: Vec<u16>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            backoff_multiplier: 2.0,
            use_jitter: true,
            retryable_status_codes: vec![502, 503, 504],
        }
    }
}

impl RetryConfig {
    /// Creates a new retry configuration with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum number of retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sets the base delay between retries.
    pub fn with_base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// Sets the maximum delay between retries.
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Sets the backoff multiplier.
    pub fn with_backoff_multiplier(mut self, multiplier: f64) -> Self {
        self.backoff_multiplier = multiplier;
        self
    }

    /// Enables or disables jitter.
    pub fn with_jitter(mut self, use_jitter: bool) -> Self {
        self.use_jitter = use_jitter;
        self
    }

    /// Sets the HTTP status codes that should trigger a retry.
    pub fn with_retryable_status_codes(mut self, codes: Vec<u16>) -> Self {
        self.retryable_status_codes = codes;
        self
    }

    /// Checks if a status code should trigger a retry.
    pub fn is_retryable_status(&self, status: u16) -> bool {
        self.retryable_status_codes.contains(&status)
    }
}

/// Retry policy that determines when and how to retry.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    config: RetryConfig,
    attempt: u32,
}

impl RetryPolicy {
    /// Creates a new retry policy with the given configuration.
    pub fn new(config: RetryConfig) -> Self {
        Self { config, attempt: 0 }
    }

    /// Returns the current attempt number (0-indexed).
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the maximum number of retries.
    pub fn max_retries(&self) -> u32 {
        self.config.max_retries
    }

    /// Checks if more retries are available.
    pub fn has_remaining_retries(&self) -> bool {
        self.attempt < self.config.max_retries
    }

    /// Calculates the delay for the next retry attempt.
    pub fn next_delay(&self) -> Duration {
        let base_ms = self.config.base_delay.as_millis() as f64;
        let multiplier = self.config.backoff_multiplier.powi(self.attempt as i32);
        let delay_ms = base_ms * multiplier;

        let delay_ms = delay_ms.min(self.config.max_delay.as_millis() as f64);

        let delay_ms = if self.config.use_jitter {
            // Add jitter: random value between 0.5x and 1.5x the delay
            let jitter = rand::thread_rng().gen_range(0.5..1.5);
            delay_ms * jitter
        } else {
            delay_ms
        };

        Duration::from_millis(delay_ms as u64)
    }

    /// Records a retry attempt and returns the delay to wait.
    ///
    /// Returns `None` if no more retries are available.
    pub fn record_retry(&mut self) -> Option<Duration> {
        if !self.has_remaining_retries() {
            return None;
        }

        let delay = self.next_delay();
        self.attempt += 1;

        debug!(
            attempt = self.attempt,
            max_retries = self.config.max_retries,
            delay_ms = delay.as_millis(),
            "scheduling retry"
        );

        Some(delay)
    }

    /// Resets the retry policy for a new request.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Checks if a response should be retried based on status code.
    pub fn should_retry_status(&self, status: u16) -> bool {
        self.has_remaining_retries() && self.config.is_retryable_status(status)
    }

    /// Checks if an error should be retried.
    ///
    /// By default, connection errors and timeouts are retryable.
    pub fn should_retry_error(&self, error: &str) -> bool {
        if !self.has_remaining_retries() {
            return false;
        }

        let error_lower = error.to_lowercase();
        error_lower.contains("connection")
            || error_lower.contains("timeout")
            || error_lower.contains("reset")
            || error_lower.contains("refused")
    }
}

/// Executes a request with retry logic.
pub struct RetryExecutor {
    policy: RetryPolicy,
}

impl RetryExecutor {
    /// Creates a new retry executor with the given configuration.
    pub fn new(config: RetryConfig) -> Self {
        Self {
            policy: RetryPolicy::new(config),
        }
    }

    /// Creates a retry executor with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RetryConfig::default())
    }

    /// Executes a request with retry logic.
    ///
    /// The `request_fn` closure is called for each attempt. If it returns
    /// a retryable error or status code, the request is retried after a delay.
    pub async fn execute<F, Fut, T, E>(&mut self, mut request_fn: F) -> Result<T, RetryError<E>>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<T, E>>,
        E: std::fmt::Display,
    {
        loop {
            match request_fn().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let error_str = e.to_string();

                    if self.policy.should_retry_error(&error_str) {
                        if let Some(delay) = self.policy.record_retry() {
                            warn!(
                                attempt = self.policy.attempt(),
                                error = %error_str,
                                delay_ms = delay.as_millis(),
                                "retrying after error"
                            );
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }

                    return Err(RetryError::Exhausted {
                        attempts: self.policy.attempt() + 1,
                        last_error: e,
                    });
                }
            }
        }
    }

    /// Resets the executor for a new request.
    pub fn reset(&mut self) {
        self.policy.reset();
    }
}

/// Error returned when retry attempts are exhausted.
#[derive(Debug)]
pub enum RetryError<E> {
    /// All retry attempts were exhausted.
    Exhausted {
        /// Total number of attempts made.
        attempts: u32,
        /// The last error encountered.
        last_error: E,
    },
}

impl<E: std::fmt::Display> std::fmt::Display for RetryError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RetryError::Exhausted {
                attempts,
                last_error,
            } => {
                write!(
                    f,
                    "all {} retry attempts exhausted, last error: {}",
                    attempts, last_error
                )
            }
        }
    }
}

impl<E: std::fmt::Debug + std::fmt::Display> std::error::Error for RetryError<E> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.base_delay, Duration::from_millis(100));
        assert!(config.use_jitter);
    }

    #[test]
    fn test_retry_config_builder() {
        let config = RetryConfig::new()
            .with_max_retries(5)
            .with_base_delay(Duration::from_millis(200))
            .with_jitter(false);

        assert_eq!(config.max_retries, 5);
        assert_eq!(config.base_delay, Duration::from_millis(200));
        assert!(!config.use_jitter);
    }

    #[test]
    fn test_retry_policy_has_remaining() {
        let config = RetryConfig::new().with_max_retries(2);
        let mut policy = RetryPolicy::new(config);

        assert!(policy.has_remaining_retries());
        policy.record_retry();
        assert!(policy.has_remaining_retries());
        policy.record_retry();
        assert!(!policy.has_remaining_retries());
    }

    #[test]
    fn test_retry_policy_delay_increases() {
        let config = RetryConfig::new()
            .with_base_delay(Duration::from_millis(100))
            .with_backoff_multiplier(2.0)
            .with_jitter(false);

        let mut policy = RetryPolicy::new(config);

        let delay1 = policy.next_delay();
        policy.record_retry();
        let delay2 = policy.next_delay();
        policy.record_retry();
        let delay3 = policy.next_delay();

        assert_eq!(delay1, Duration::from_millis(100));
        assert_eq!(delay2, Duration::from_millis(200));
        assert_eq!(delay3, Duration::from_millis(400));
    }

    #[test]
    fn test_retry_policy_max_delay() {
        let config = RetryConfig::new()
            .with_base_delay(Duration::from_secs(1))
            .with_max_delay(Duration::from_secs(5))
            .with_backoff_multiplier(10.0)
            .with_jitter(false);

        let mut policy = RetryPolicy::new(config);
        policy.record_retry();
        policy.record_retry();

        let delay = policy.next_delay();
        assert_eq!(delay, Duration::from_secs(5));
    }

    #[test]
    fn test_retry_policy_retryable_status() {
        let config = RetryConfig::new().with_retryable_status_codes(vec![502, 503]);
        let policy = RetryPolicy::new(config);

        assert!(policy.should_retry_status(502));
        assert!(policy.should_retry_status(503));
        assert!(!policy.should_retry_status(500));
        assert!(!policy.should_retry_status(200));
    }

    #[test]
    fn test_retry_policy_retryable_error() {
        let config = RetryConfig::new();
        let policy = RetryPolicy::new(config);

        assert!(policy.should_retry_error("connection refused"));
        assert!(policy.should_retry_error("Connection reset by peer"));
        assert!(policy.should_retry_error("request timeout"));
        assert!(!policy.should_retry_error("invalid request"));
    }

    #[tokio::test]
    async fn test_retry_executor_success() {
        let mut executor = RetryExecutor::with_defaults();
        let result = executor
            .execute(|| async { Ok::<i32, &str>(42) })
            .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_executor_exhausted() {
        let config = RetryConfig::new().with_max_retries(2);
        let mut executor = RetryExecutor::new(config);

        let result: Result<i32, RetryError<&str>> = executor
            .execute(|| async { Err("connection refused") })
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            RetryError::Exhausted { attempts, .. } => {
                assert_eq!(attempts, 3); // 1 initial + 2 retries
            }
        }
    }
}
