//! Connection pooling and load balancing for upstream connections.
//!
//! Provides efficient connection management with support for multiple
//! load balancing strategies and health-aware routing.

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::router::LoadBalancingPolicy;
use dashmap::DashMap;
use parking_lot::RwLock;
use rand::Rng;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

/// Configuration for the connection pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of idle connections per host.
    pub max_idle_per_host: usize,
    /// Maximum total connections per host.
    pub max_connections_per_host: usize,
    /// Idle connection timeout.
    pub idle_timeout: Duration,
    /// Connection establishment timeout.
    pub connect_timeout: Duration,
    /// Enable HTTP/2 connection pooling.
    pub http2_only: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_idle_per_host: 10,
            max_connections_per_host: 100,
            idle_timeout: Duration::from_secs(90),
            connect_timeout: Duration::from_secs(10),
            http2_only: false,
        }
    }
}

impl PoolConfig {
    /// Creates a new pool configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the maximum idle connections per host.
    pub fn with_max_idle(mut self, max: usize) -> Self {
        self.max_idle_per_host = max;
        self
    }

    /// Sets the idle timeout.
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Enables HTTP/2 only mode.
    pub fn with_http2_only(mut self, http2: bool) -> Self {
        self.http2_only = http2;
        self
    }
}

/// Statistics for a single endpoint.
#[derive(Debug, Clone)]
pub struct EndpointStats {
    /// Total number of requests sent.
    pub total_requests: u64,
    /// Number of successful requests.
    pub successful_requests: u64,
    /// Number of failed requests.
    pub failed_requests: u64,
    /// Current active connections.
    pub active_connections: usize,
    /// Average response time in milliseconds.
    pub avg_response_time_ms: f64,
    /// Whether the endpoint is healthy.
    pub is_healthy: bool,
}

/// A single upstream endpoint.
#[derive(Debug)]
pub struct Endpoint {
    /// The endpoint address (e.g., "http://localhost:8080").
    pub address: String,
    /// Current weight for weighted load balancing.
    weight: AtomicU64,
    /// Number of active connections.
    active_connections: AtomicUsize,
    /// Total request count.
    total_requests: AtomicU64,
    /// Successful request count.
    successful_requests: AtomicU64,
    /// Failed request count.
    failed_requests: AtomicU64,
    /// Sum of response times in microseconds.
    total_response_time_us: AtomicU64,
    /// Whether the endpoint is healthy.
    healthy: RwLock<bool>,
    /// Circuit breaker for this endpoint.
    circuit_breaker: CircuitBreaker,
    /// Last health check time (used for periodic health checks).
    #[allow(dead_code)]
    last_health_check: RwLock<Instant>,
}

impl Endpoint {
    /// Creates a new endpoint.
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            weight: AtomicU64::new(100),
            active_connections: AtomicUsize::new(0),
            total_requests: AtomicU64::new(0),
            successful_requests: AtomicU64::new(0),
            failed_requests: AtomicU64::new(0),
            total_response_time_us: AtomicU64::new(0),
            healthy: RwLock::new(true),
            circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
            last_health_check: RwLock::new(Instant::now()),
        }
    }

    /// Creates a new endpoint with the given weight.
    pub fn with_weight(address: impl Into<String>, weight: u64) -> Self {
        let endpoint = Self::new(address);
        endpoint.weight.store(weight, Ordering::Relaxed);
        endpoint
    }

    /// Returns the current weight.
    pub fn weight(&self) -> u64 {
        self.weight.load(Ordering::Relaxed)
    }

    /// Sets the weight.
    pub fn set_weight(&self, weight: u64) {
        self.weight.store(weight, Ordering::Relaxed);
    }

    /// Returns the number of active connections.
    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    /// Increments the active connection count.
    pub fn acquire_connection(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the active connection count.
    pub fn release_connection(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    /// Returns whether the endpoint is healthy.
    pub fn is_healthy(&self) -> bool {
        *self.healthy.read()
    }

    /// Sets the health status.
    pub fn set_healthy(&self, healthy: bool) {
        *self.healthy.write() = healthy;
    }

    /// Records a successful request.
    pub async fn record_success(&self, response_time: Duration) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful_requests.fetch_add(1, Ordering::Relaxed);
        self.total_response_time_us
            .fetch_add(response_time.as_micros() as u64, Ordering::Relaxed);
        self.circuit_breaker.record_success().await;
    }

    /// Records a failed request.
    pub async fn record_failure(&self) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.failed_requests.fetch_add(1, Ordering::Relaxed);
        self.circuit_breaker.record_failure().await;
    }

    /// Checks if a request should be allowed through the circuit breaker.
    pub async fn allow_request(&self) -> bool {
        self.circuit_breaker.allow_request().await
    }

    /// Returns statistics for this endpoint.
    pub fn stats(&self) -> EndpointStats {
        let total = self.total_requests.load(Ordering::Relaxed);
        let total_time = self.total_response_time_us.load(Ordering::Relaxed);
        let avg_time = if total > 0 {
            (total_time as f64 / total as f64) / 1000.0
        } else {
            0.0
        };

        EndpointStats {
            total_requests: total,
            successful_requests: self.successful_requests.load(Ordering::Relaxed),
            failed_requests: self.failed_requests.load(Ordering::Relaxed),
            active_connections: self.active_connections.load(Ordering::Relaxed),
            avg_response_time_ms: avg_time,
            is_healthy: *self.healthy.read(),
        }
    }
}

/// Load balancer for distributing requests across endpoints.
pub struct LoadBalancer {
    endpoints: Vec<Arc<Endpoint>>,
    policy: LoadBalancingPolicy,
    next_index: AtomicUsize,
}

impl LoadBalancer {
    /// Creates a new load balancer.
    pub fn new(endpoints: Vec<Arc<Endpoint>>, policy: LoadBalancingPolicy) -> Self {
        Self {
            endpoints,
            policy,
            next_index: AtomicUsize::new(0),
        }
    }

    /// Creates a load balancer from endpoint addresses.
    pub fn from_addresses(addresses: Vec<String>, policy: LoadBalancingPolicy) -> Self {
        let endpoints = addresses
            .into_iter()
            .map(|addr| Arc::new(Endpoint::new(addr)))
            .collect();
        Self::new(endpoints, policy)
    }

    /// Selects the next endpoint based on the load balancing policy.
    pub async fn select(&self) -> Option<Arc<Endpoint>> {
        let healthy_endpoints: Vec<_> = self
            .endpoints
            .iter()
            .filter(|e| e.is_healthy())
            .cloned()
            .collect();

        if healthy_endpoints.is_empty() {
            warn!("no healthy endpoints available");
            return None;
        }

        let endpoint = match self.policy {
            LoadBalancingPolicy::RoundRobin => self.round_robin(&healthy_endpoints),
            LoadBalancingPolicy::LeastConnections => self.least_connections(&healthy_endpoints),
            LoadBalancingPolicy::Random => self.random(&healthy_endpoints),
            LoadBalancingPolicy::ConsistentHash => {
                // Fallback to round-robin for now
                self.round_robin(&healthy_endpoints)
            }
        };

        // Check circuit breaker
        if let Some(ref ep) = endpoint {
            if !ep.allow_request().await {
                warn!(endpoint = %ep.address, "circuit breaker is open");
                // Try to find another endpoint
                for e in &healthy_endpoints {
                    if e.address != ep.address && e.allow_request().await {
                        return Some(e.clone());
                    }
                }
                return None;
            }
        }

        endpoint
    }

    /// Round-robin selection.
    fn round_robin(&self, endpoints: &[Arc<Endpoint>]) -> Option<Arc<Endpoint>> {
        if endpoints.is_empty() {
            return None;
        }
        let idx = self.next_index.fetch_add(1, Ordering::Relaxed) % endpoints.len();
        Some(endpoints[idx].clone())
    }

    /// Least connections selection.
    fn least_connections(&self, endpoints: &[Arc<Endpoint>]) -> Option<Arc<Endpoint>> {
        endpoints
            .iter()
            .min_by_key(|e| e.active_connections())
            .cloned()
    }

    /// Random selection.
    fn random(&self, endpoints: &[Arc<Endpoint>]) -> Option<Arc<Endpoint>> {
        if endpoints.is_empty() {
            return None;
        }
        let idx = rand::thread_rng().gen_range(0..endpoints.len());
        Some(endpoints[idx].clone())
    }

    /// Returns all endpoints.
    pub fn endpoints(&self) -> &[Arc<Endpoint>] {
        &self.endpoints
    }

    /// Returns the number of healthy endpoints.
    pub fn healthy_count(&self) -> usize {
        self.endpoints.iter().filter(|e| e.is_healthy()).count()
    }

    /// Returns the total number of endpoints.
    pub fn total_count(&self) -> usize {
        self.endpoints.len()
    }
}

/// Connection pool statistics.
#[derive(Debug, Clone)]
pub struct PoolStats {
    /// Total number of connections created.
    pub connections_created: u64,
    /// Total number of connections closed.
    pub connections_closed: u64,
    /// Current number of idle connections.
    pub idle_connections: usize,
    /// Current number of active connections.
    pub active_connections: usize,
}

/// Transport layer managing connection pools for upstream clusters.
pub struct Transport {
    /// Pool configuration.
    config: PoolConfig,
    /// Load balancers for each cluster.
    clusters: DashMap<String, Arc<LoadBalancer>>,
    /// Connection statistics.
    stats: Arc<TransportStats>,
}

/// Transport statistics.
struct TransportStats {
    connections_created: AtomicU64,
    connections_closed: AtomicU64,
}

impl Transport {
    /// Creates a new transport with the given configuration.
    pub fn new(config: PoolConfig) -> Self {
        Self {
            config,
            clusters: DashMap::new(),
            stats: Arc::new(TransportStats {
                connections_created: AtomicU64::new(0),
                connections_closed: AtomicU64::new(0),
            }),
        }
    }

    /// Creates a transport with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(PoolConfig::default())
    }

    /// Adds a cluster with the given endpoints.
    pub fn add_cluster(
        &self,
        name: impl Into<String>,
        endpoints: Vec<String>,
        policy: LoadBalancingPolicy,
    ) {
        let lb = LoadBalancer::from_addresses(endpoints, policy);
        self.clusters.insert(name.into(), Arc::new(lb));
    }

    /// Gets a load balancer for a cluster.
    pub fn get_cluster(&self, name: &str) -> Option<Arc<LoadBalancer>> {
        self.clusters.get(name).map(|r| r.clone())
    }

    /// Selects an endpoint from a cluster.
    pub async fn select_endpoint(&self, cluster: &str) -> Option<Arc<Endpoint>> {
        if let Some(lb) = self.get_cluster(cluster) {
            lb.select().await
        } else {
            warn!(cluster = %cluster, "cluster not found");
            None
        }
    }

    /// Returns the pool configuration.
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Returns pool statistics.
    pub fn stats(&self) -> PoolStats {
        let mut active = 0;

        for cluster in self.clusters.iter() {
            for endpoint in cluster.endpoints() {
                active += endpoint.active_connections();
            }
        }

        PoolStats {
            connections_created: self.stats.connections_created.load(Ordering::Relaxed),
            connections_closed: self.stats.connections_closed.load(Ordering::Relaxed),
            idle_connections: 0, // TODO: implement idle connection tracking
            active_connections: active,
        }
    }

    /// Records a connection being created.
    pub fn record_connection_created(&self) {
        self.stats
            .connections_created
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Records a connection being closed.
    pub fn record_connection_closed(&self) {
        self.stats
            .connections_closed
            .fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_basic() {
        let endpoint = Endpoint::new("http://localhost:8080");
        assert!(endpoint.is_healthy());
        assert_eq!(endpoint.active_connections(), 0);
        assert_eq!(endpoint.weight(), 100);
    }

    #[test]
    fn test_endpoint_with_weight() {
        let endpoint = Endpoint::with_weight("http://localhost:8080", 50);
        assert_eq!(endpoint.weight(), 50);
    }

    #[test]
    fn test_endpoint_connections() {
        let endpoint = Endpoint::new("http://localhost:8080");
        endpoint.acquire_connection();
        endpoint.acquire_connection();
        assert_eq!(endpoint.active_connections(), 2);

        endpoint.release_connection();
        assert_eq!(endpoint.active_connections(), 1);
    }

    #[tokio::test]
    async fn test_endpoint_stats() {
        let endpoint = Endpoint::new("http://localhost:8080");
        endpoint.record_success(Duration::from_millis(100)).await;
        endpoint.record_success(Duration::from_millis(200)).await;
        endpoint.record_failure().await;

        let stats = endpoint.stats();
        assert_eq!(stats.total_requests, 3);
        assert_eq!(stats.successful_requests, 2);
        assert_eq!(stats.failed_requests, 1);
        assert!(stats.avg_response_time_ms > 0.0);
    }

    #[tokio::test]
    async fn test_load_balancer_round_robin() {
        let lb = LoadBalancer::from_addresses(
            vec![
                "http://host1:8080".to_string(),
                "http://host2:8080".to_string(),
                "http://host3:8080".to_string(),
            ],
            LoadBalancingPolicy::RoundRobin,
        );

        let ep1 = lb.select().await.unwrap();
        let ep2 = lb.select().await.unwrap();
        let ep3 = lb.select().await.unwrap();
        let ep4 = lb.select().await.unwrap();

        // Should cycle through all endpoints
        assert_ne!(ep1.address, ep2.address);
        assert_ne!(ep2.address, ep3.address);
        assert_eq!(ep1.address, ep4.address); // Back to first
    }

    #[tokio::test]
    async fn test_load_balancer_least_connections() {
        let endpoints = vec![
            Arc::new(Endpoint::new("http://host1:8080")),
            Arc::new(Endpoint::new("http://host2:8080")),
        ];

        // Add connections to host1
        endpoints[0].acquire_connection();
        endpoints[0].acquire_connection();

        let lb = LoadBalancer::new(endpoints, LoadBalancingPolicy::LeastConnections);

        // Should select host2 (fewer connections)
        let selected = lb.select().await.unwrap();
        assert_eq!(selected.address, "http://host2:8080");
    }

    #[tokio::test]
    async fn test_load_balancer_unhealthy_skip() {
        let endpoints = vec![
            Arc::new(Endpoint::new("http://host1:8080")),
            Arc::new(Endpoint::new("http://host2:8080")),
        ];

        // Mark host1 as unhealthy
        endpoints[0].set_healthy(false);

        let lb = LoadBalancer::new(endpoints, LoadBalancingPolicy::RoundRobin);

        // Should always select host2
        for _ in 0..5 {
            let selected = lb.select().await.unwrap();
            assert_eq!(selected.address, "http://host2:8080");
        }
    }

    #[test]
    fn test_transport_add_cluster() {
        let transport = Transport::with_defaults();
        transport.add_cluster(
            "api",
            vec![
                "http://api1:8080".to_string(),
                "http://api2:8080".to_string(),
            ],
            LoadBalancingPolicy::RoundRobin,
        );

        let cluster = transport.get_cluster("api");
        assert!(cluster.is_some());
        assert_eq!(cluster.unwrap().total_count(), 2);
    }

    #[tokio::test]
    async fn test_transport_select_endpoint() {
        let transport = Transport::with_defaults();
        transport.add_cluster(
            "api",
            vec!["http://api1:8080".to_string()],
            LoadBalancingPolicy::RoundRobin,
        );

        let endpoint = transport.select_endpoint("api").await;
        assert!(endpoint.is_some());
        assert_eq!(endpoint.unwrap().address, "http://api1:8080");

        let missing = transport.select_endpoint("nonexistent").await;
        assert!(missing.is_none());
    }
}
