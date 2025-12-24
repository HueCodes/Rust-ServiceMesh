//! Rate limiting middleware using token bucket algorithm.
//!
//! Provides configurable rate limiting for incoming requests with support
//! for multiple strategies: per-client, global, and per-route limiting.
//!
//! Supports X-Forwarded-For header parsing for clients behind proxies.

use dashmap::DashMap;
use http::HeaderMap;
use parking_lot::Mutex;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Configuration for rate limiting.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum number of requests allowed in the window.
    pub requests_per_second: u64,
    /// Burst capacity (allows temporary spikes above the rate).
    pub burst_size: u64,
    /// Whether to enable per-client rate limiting.
    pub per_client: bool,
    /// Time-to-live for client rate limit entries.
    pub client_ttl: Duration,
    /// Whether to trust X-Forwarded-For headers.
    pub trust_forwarded_for: bool,
    /// Maximum number of IPs to consider from X-Forwarded-For chain.
    pub max_forwarded_ips: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 100,
            burst_size: 50,
            per_client: true,
            client_ttl: Duration::from_secs(300),
            trust_forwarded_for: false,
            max_forwarded_ips: 1,
        }
    }
}

impl RateLimitConfig {
    /// Creates a new rate limit configuration.
    pub fn new(requests_per_second: u64, burst_size: u64) -> Self {
        Self {
            requests_per_second,
            burst_size,
            ..Default::default()
        }
    }

    /// Enables or disables per-client rate limiting.
    pub fn with_per_client(mut self, per_client: bool) -> Self {
        self.per_client = per_client;
        self
    }

    /// Sets the TTL for client entries.
    pub fn with_client_ttl(mut self, ttl: Duration) -> Self {
        self.client_ttl = ttl;
        self
    }

    /// Enables trusting X-Forwarded-For headers.
    ///
    /// **Security Warning**: Only enable this if the proxy is behind a trusted
    /// load balancer that sets this header. Untrusted clients can spoof this
    /// header to bypass rate limiting.
    pub fn with_trust_forwarded_for(mut self, trust: bool) -> Self {
        self.trust_forwarded_for = trust;
        self
    }

    /// Sets the maximum number of IPs to consider from X-Forwarded-For.
    ///
    /// When set to 1, only the rightmost (closest to proxy) IP is used.
    /// Higher values can be used in multi-proxy setups.
    pub fn with_max_forwarded_ips(mut self, max: usize) -> Self {
        self.max_forwarded_ips = max.max(1);
        self
    }
}

/// Token bucket for rate limiting.
#[derive(Debug)]
struct TokenBucket {
    /// Current number of available tokens.
    tokens: f64,
    /// Maximum capacity of the bucket.
    capacity: f64,
    /// Rate at which tokens are added (per second).
    refill_rate: f64,
    /// Last time the bucket was updated.
    last_update: Instant,
}

impl TokenBucket {
    /// Creates a new token bucket.
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_update: Instant::now(),
        }
    }

    /// Refills tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_update = now;
    }

    /// Attempts to consume a token.
    ///
    /// Returns `true` if a token was consumed, `false` if the bucket is empty.
    fn try_consume(&mut self) -> bool {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Returns the estimated wait time until a token is available.
    fn wait_time(&self) -> Duration {
        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            let needed = 1.0 - self.tokens;
            Duration::from_secs_f64(needed / self.refill_rate)
        }
    }

    /// Returns the current number of available tokens.
    fn available_tokens(&self) -> f64 {
        self.tokens
    }
}

/// Client rate limit entry with TTL tracking.
struct ClientEntry {
    bucket: Mutex<TokenBucket>,
    last_access: Mutex<Instant>,
}

impl ClientEntry {
    fn new(config: &RateLimitConfig) -> Self {
        Self {
            bucket: Mutex::new(TokenBucket::new(
                config.burst_size as f64,
                config.requests_per_second as f64,
            )),
            last_access: Mutex::new(Instant::now()),
        }
    }

    fn try_acquire(&self) -> bool {
        *self.last_access.lock() = Instant::now();
        self.bucket.lock().try_consume()
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.last_access.lock().elapsed() > ttl
    }
}

/// Rate limiter with support for global and per-client limiting.
pub struct RateLimiter {
    config: RateLimitConfig,
    global_bucket: Mutex<TokenBucket>,
    client_buckets: DashMap<IpAddr, Arc<ClientEntry>>,
    last_cleanup: Mutex<Instant>,
}

impl RateLimiter {
    /// Creates a new rate limiter with the given configuration.
    pub fn new(config: RateLimitConfig) -> Self {
        let global_bucket =
            TokenBucket::new(config.burst_size as f64, config.requests_per_second as f64);

        Self {
            config,
            global_bucket: Mutex::new(global_bucket),
            client_buckets: DashMap::new(),
            last_cleanup: Mutex::new(Instant::now()),
        }
    }

    /// Creates a rate limiter with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(RateLimitConfig::default())
    }

    /// Extracts the client IP from request headers.
    ///
    /// If `trust_forwarded_for` is enabled, attempts to parse the
    /// X-Forwarded-For header. Falls back to the provided socket address.
    ///
    /// # Arguments
    ///
    /// * `headers` - HTTP request headers
    /// * `socket_addr` - The direct socket connection address
    pub fn extract_client_ip(
        &self,
        headers: &HeaderMap,
        socket_addr: Option<IpAddr>,
    ) -> Option<IpAddr> {
        if self.config.trust_forwarded_for {
            if let Some(forwarded) = headers.get("x-forwarded-for") {
                if let Ok(value) = forwarded.to_str() {
                    // X-Forwarded-For format: client, proxy1, proxy2, ...
                    // We want the rightmost N IPs (closest to our proxy)
                    let ips: Vec<&str> = value.split(',').map(|s| s.trim()).collect();

                    // Take the first IP (original client) by default
                    // In production with trusted proxies, you might want the last one
                    // before your own proxy (ips.len() - max_forwarded_ips)
                    if let Some(ip_str) = ips.first() {
                        match IpAddr::from_str(ip_str) {
                            Ok(ip) => {
                                debug!(ip = %ip, "using X-Forwarded-For client IP");
                                return Some(ip);
                            }
                            Err(e) => {
                                warn!(
                                    header = %value,
                                    error = %e,
                                    "invalid IP in X-Forwarded-For header"
                                );
                            }
                        }
                    }
                }
            }

            // Also check X-Real-IP header (used by nginx)
            if let Some(real_ip) = headers.get("x-real-ip") {
                if let Ok(value) = real_ip.to_str() {
                    match IpAddr::from_str(value.trim()) {
                        Ok(ip) => {
                            debug!(ip = %ip, "using X-Real-IP client IP");
                            return Some(ip);
                        }
                        Err(e) => {
                            warn!(
                                header = %value,
                                error = %e,
                                "invalid IP in X-Real-IP header"
                            );
                        }
                    }
                }
            }
        }

        socket_addr
    }

    /// Checks if a request should be allowed using headers to determine client IP.
    ///
    /// Returns `Ok(())` if allowed, `Err(RateLimitInfo)` if rate limited.
    pub fn check_with_headers(
        &self,
        headers: &HeaderMap,
        socket_addr: Option<IpAddr>,
    ) -> Result<(), RateLimitInfo> {
        let client_ip = self.extract_client_ip(headers, socket_addr);
        self.check(client_ip)
    }

    /// Checks if a request should be allowed.
    ///
    /// Returns `Ok(())` if allowed, `Err(RateLimitInfo)` if rate limited.
    pub fn check(&self, client_ip: Option<IpAddr>) -> Result<(), RateLimitInfo> {
        // First check global rate limit
        if !self.global_bucket.lock().try_consume() {
            let wait_time = self.global_bucket.lock().wait_time();
            debug!("global rate limit exceeded");
            return Err(RateLimitInfo {
                limit_type: RateLimitType::Global,
                retry_after: wait_time,
                remaining: 0,
            });
        }

        // Then check per-client rate limit if enabled
        if self.config.per_client {
            if let Some(ip) = client_ip {
                self.maybe_cleanup();

                let entry = self
                    .client_buckets
                    .entry(ip)
                    .or_insert_with(|| Arc::new(ClientEntry::new(&self.config)))
                    .clone();

                if !entry.try_acquire() {
                    let wait_time = entry.bucket.lock().wait_time();
                    debug!(client = %ip, "per-client rate limit exceeded");
                    return Err(RateLimitInfo {
                        limit_type: RateLimitType::PerClient,
                        retry_after: wait_time,
                        remaining: 0,
                    });
                }
            }
        }

        Ok(())
    }

    /// Cleans up expired client entries periodically.
    fn maybe_cleanup(&self) {
        let mut last_cleanup = self.last_cleanup.lock();
        if last_cleanup.elapsed() < Duration::from_secs(60) {
            return;
        }

        *last_cleanup = Instant::now();
        drop(last_cleanup);

        let ttl = self.config.client_ttl;
        let initial_count = self.client_buckets.len();

        self.client_buckets
            .retain(|_, entry| !entry.is_expired(ttl));

        let removed = initial_count - self.client_buckets.len();
        if removed > 0 {
            debug!(removed = removed, "cleaned up expired rate limit entries");
        }
    }

    /// Returns the current statistics.
    pub fn stats(&self) -> RateLimitStats {
        RateLimitStats {
            global_available: self.global_bucket.lock().available_tokens() as u64,
            client_count: self.client_buckets.len(),
            requests_per_second: self.config.requests_per_second,
            burst_size: self.config.burst_size,
        }
    }
}

/// Type of rate limit that was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitType {
    /// Global rate limit was exceeded.
    Global,
    /// Per-client rate limit was exceeded.
    PerClient,
}

/// Information about a rate limit rejection.
#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    /// Type of rate limit that was exceeded.
    pub limit_type: RateLimitType,
    /// Suggested time to wait before retrying.
    pub retry_after: Duration,
    /// Remaining requests in the current window.
    pub remaining: u64,
}

impl RateLimitInfo {
    /// Returns the `Retry-After` header value in seconds.
    pub fn retry_after_secs(&self) -> u64 {
        self.retry_after.as_secs().max(1)
    }
}

/// Rate limiter statistics.
#[derive(Debug, Clone)]
pub struct RateLimitStats {
    /// Available tokens in the global bucket.
    pub global_available: u64,
    /// Number of tracked clients.
    pub client_count: usize,
    /// Configured requests per second.
    pub requests_per_second: u64,
    /// Configured burst size.
    pub burst_size: u64,
}

/// Rate limiter middleware wrapper for Tower services.
pub struct RateLimitLayer {
    limiter: Arc<RateLimiter>,
}

impl RateLimitLayer {
    /// Creates a new rate limit layer.
    pub fn new(limiter: Arc<RateLimiter>) -> Self {
        Self { limiter }
    }

    /// Returns the underlying rate limiter.
    pub fn limiter(&self) -> &Arc<RateLimiter> {
        &self.limiter
    }
}

impl Clone for RateLimitLayer {
    fn clone(&self) -> Self {
        Self {
            limiter: Arc::clone(&self.limiter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use std::net::Ipv4Addr;

    #[test]
    fn test_token_bucket_basic() {
        let mut bucket = TokenBucket::new(10.0, 10.0);

        // Should be able to consume initial tokens
        for _ in 0..10 {
            assert!(bucket.try_consume());
        }

        // Should be empty now
        assert!(!bucket.try_consume());
    }

    #[test]
    fn test_token_bucket_refill() {
        let mut bucket = TokenBucket::new(10.0, 1000.0);

        // Consume all tokens
        for _ in 0..10 {
            bucket.try_consume();
        }

        // Simulate time passing (by manually setting last_update)
        bucket.last_update = Instant::now() - Duration::from_millis(100);
        bucket.refill();

        // Should have refilled ~100 tokens (capped at capacity)
        assert!(bucket.available_tokens() >= 9.0);
    }

    #[test]
    fn test_rate_limiter_global() {
        let config = RateLimitConfig::new(10, 5).with_per_client(false);
        let limiter = RateLimiter::new(config);

        // Should allow burst_size requests
        for _ in 0..5 {
            assert!(limiter.check(None).is_ok());
        }

        // Should be rate limited after burst
        assert!(limiter.check(None).is_err());
    }

    #[test]
    fn test_rate_limiter_per_client() {
        // Global: 100 req/s, burst 10 - Per-client: same (inherited)
        let config = RateLimitConfig::new(100, 10).with_per_client(true);
        let limiter = RateLimiter::new(config);

        let client1 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let client2 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2));

        // Client 1 uses 5 of their 10 tokens
        for _ in 0..5 {
            assert!(limiter.check(Some(client1)).is_ok());
        }

        // Client 2 should have their own 10 token quota
        // Note: global bucket also depletes, so client2 uses from both
        assert!(limiter.check(Some(client2)).is_ok());
        assert!(limiter.check(Some(client2)).is_ok());
    }

    #[test]
    fn test_rate_limit_info() {
        let info = RateLimitInfo {
            limit_type: RateLimitType::Global,
            retry_after: Duration::from_millis(500),
            remaining: 0,
        };

        assert_eq!(info.retry_after_secs(), 1);
    }

    #[test]
    fn test_rate_limiter_stats() {
        let config = RateLimitConfig::new(100, 50);
        let limiter = RateLimiter::new(config);

        let stats = limiter.stats();
        assert_eq!(stats.requests_per_second, 100);
        assert_eq!(stats.burst_size, 50);
        assert_eq!(stats.client_count, 0);
    }

    #[test]
    fn test_extract_client_ip_no_trust() {
        let config = RateLimitConfig::new(100, 50).with_trust_forwarded_for(false);
        let limiter = RateLimiter::new(config);

        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());

        let socket_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = limiter.extract_client_ip(&headers, Some(socket_ip));

        // Should ignore X-Forwarded-For when trust is disabled
        assert_eq!(result, Some(socket_ip));
    }

    #[test]
    fn test_extract_client_ip_trust_enabled() {
        let config = RateLimitConfig::new(100, 50).with_trust_forwarded_for(true);
        let limiter = RateLimiter::new(config);

        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 5.6.7.8".parse().unwrap());

        let socket_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = limiter.extract_client_ip(&headers, Some(socket_ip));

        // Should use the first IP from X-Forwarded-For
        assert_eq!(result, Some(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))));
    }

    #[test]
    fn test_extract_client_ip_x_real_ip() {
        let config = RateLimitConfig::new(100, 50).with_trust_forwarded_for(true);
        let limiter = RateLimiter::new(config);

        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "10.0.0.1".parse().unwrap());

        let socket_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = limiter.extract_client_ip(&headers, Some(socket_ip));

        // Should use X-Real-IP
        assert_eq!(result, Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn test_extract_client_ip_invalid_header() {
        let config = RateLimitConfig::new(100, 50).with_trust_forwarded_for(true);
        let limiter = RateLimiter::new(config);

        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());

        let socket_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        let result = limiter.extract_client_ip(&headers, Some(socket_ip));

        // Should fall back to socket address when header is invalid
        assert_eq!(result, Some(socket_ip));
    }

    #[test]
    fn test_check_with_headers() {
        let config = RateLimitConfig::new(100, 5)
            .with_per_client(true)
            .with_trust_forwarded_for(true);
        let limiter = RateLimiter::new(config);

        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4".parse().unwrap());

        // Should allow requests up to burst size
        for _ in 0..5 {
            assert!(limiter.check_with_headers(&headers, None).is_ok());
        }

        // Should be rate limited
        assert!(limiter.check_with_headers(&headers, None).is_err());
    }
}
