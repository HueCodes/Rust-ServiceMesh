//! Prometheus metrics collection and export.
//!
//! Provides comprehensive observability following RED methodology:
//! - **Rate**: Request rate and throughput
//! - **Errors**: Error counts and error rates
//! - **Duration**: Request latency histograms

use once_cell::sync::Lazy;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use std::io;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

/// Labels for HTTP request metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    /// HTTP method (GET, POST, etc.)
    pub method: String,
    /// HTTP status code (200, 404, etc.)
    pub status: String,
    /// Upstream address
    pub upstream: String,
}

/// Labels for circuit breaker metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct CircuitBreakerLabels {
    /// Upstream or endpoint name
    pub upstream: String,
    /// Circuit breaker state (closed, open, half_open)
    pub state: String,
}

/// Labels for rate limiter metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RateLimitLabels {
    /// Type of rate limit (global, per_client)
    pub limit_type: String,
}

/// Labels for connection metrics.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ConnectionLabels {
    /// Connection state or type
    pub state: String,
}

/// Global metrics registry.
///
/// Initialized once at startup and shared across all tasks.
static METRICS: Lazy<Arc<Mutex<Metrics>>> = Lazy::new(|| Arc::new(Mutex::new(Metrics::new())));

/// Metrics collector for the proxy.
///
/// Tracks request counts, latencies, circuit breaker states, rate limits,
/// and connection statistics following RED methodology.
pub struct Metrics {
    registry: Registry,
    // Request metrics (RED)
    requests_total: Family<HttpLabels, Counter>,
    request_duration_seconds: Family<HttpLabels, Histogram>,
    requests_in_flight: Gauge<i64, AtomicI64>,
    // Error metrics
    errors_total: Family<HttpLabels, Counter>,
    // Circuit breaker metrics
    circuit_breaker_state: Family<CircuitBreakerLabels, Gauge>,
    circuit_breaker_trips_total: Family<CircuitBreakerLabels, Counter>,
    // Rate limiter metrics
    rate_limit_rejections_total: Family<RateLimitLabels, Counter>,
    // Connection metrics
    connections_total: Family<ConnectionLabels, Counter>,
    connections_active: Gauge<i64, AtomicI64>,
}

impl Metrics {
    /// Creates a new metrics collector with default buckets.
    fn new() -> Self {
        let mut registry = Registry::default();

        // Request metrics
        let requests_total = Family::<HttpLabels, Counter>::default();
        registry.register(
            "http_requests_total",
            "Total number of HTTP requests",
            requests_total.clone(),
        );

        let request_duration_seconds =
            Family::<HttpLabels, Histogram>::new_with_constructor(|| {
                // Buckets: 1ms, 2ms, 4ms, 8ms, 16ms, 32ms, 64ms, 128ms, 256ms, 512ms, 1s, 2s, 4s
                Histogram::new(exponential_buckets(0.001, 2.0, 13))
            });
        registry.register(
            "http_request_duration_seconds",
            "HTTP request latency in seconds",
            request_duration_seconds.clone(),
        );

        let requests_in_flight = Gauge::<i64, AtomicI64>::default();
        registry.register(
            "http_requests_in_flight",
            "Number of HTTP requests currently being processed",
            requests_in_flight.clone(),
        );

        // Error metrics
        let errors_total = Family::<HttpLabels, Counter>::default();
        registry.register(
            "http_errors_total",
            "Total number of HTTP errors (4xx and 5xx responses)",
            errors_total.clone(),
        );

        // Circuit breaker metrics
        let circuit_breaker_state = Family::<CircuitBreakerLabels, Gauge>::default();
        registry.register(
            "circuit_breaker_state",
            "Current state of circuit breakers (0=closed, 1=open, 2=half_open)",
            circuit_breaker_state.clone(),
        );

        let circuit_breaker_trips_total = Family::<CircuitBreakerLabels, Counter>::default();
        registry.register(
            "circuit_breaker_trips_total",
            "Total number of times circuit breakers have tripped",
            circuit_breaker_trips_total.clone(),
        );

        // Rate limiter metrics
        let rate_limit_rejections_total = Family::<RateLimitLabels, Counter>::default();
        registry.register(
            "rate_limit_rejections_total",
            "Total number of requests rejected due to rate limiting",
            rate_limit_rejections_total.clone(),
        );

        // Connection metrics
        let connections_total = Family::<ConnectionLabels, Counter>::default();
        registry.register(
            "connections_total",
            "Total number of connections by state",
            connections_total.clone(),
        );

        let connections_active = Gauge::<i64, AtomicI64>::default();
        registry.register(
            "connections_active",
            "Number of currently active connections",
            connections_active.clone(),
        );

        Self {
            registry,
            requests_total,
            request_duration_seconds,
            requests_in_flight,
            errors_total,
            circuit_breaker_state,
            circuit_breaker_trips_total,
            rate_limit_rejections_total,
            connections_total,
            connections_active,
        }
    }

    /// Records an HTTP request with method, status, upstream, and duration.
    ///
    /// # Arguments
    ///
    /// * `method` - HTTP method (e.g., "GET", "POST")
    /// * `status` - HTTP status code (e.g., 200, 404)
    /// * `upstream` - Upstream server address
    /// * `duration_secs` - Request duration in seconds
    pub fn record_request(method: &str, status: u16, upstream: &str, duration_secs: f64) {
        let labels = HttpLabels {
            method: method.to_string(),
            status: status.to_string(),
            upstream: upstream.to_string(),
        };

        if let Ok(metrics) = METRICS.lock() {
            metrics.requests_total.get_or_create(&labels).inc();
            metrics
                .request_duration_seconds
                .get_or_create(&labels)
                .observe(duration_secs);

            // Track errors (4xx and 5xx)
            if status >= 400 {
                metrics.errors_total.get_or_create(&labels).inc();
            }
        }
    }

    /// Increments the in-flight request counter.
    pub fn inc_requests_in_flight() {
        if let Ok(metrics) = METRICS.lock() {
            metrics.requests_in_flight.inc();
        }
    }

    /// Decrements the in-flight request counter.
    pub fn dec_requests_in_flight() {
        if let Ok(metrics) = METRICS.lock() {
            metrics.requests_in_flight.dec();
        }
    }

    /// Records a circuit breaker state change.
    ///
    /// # Arguments
    ///
    /// * `upstream` - The upstream endpoint name
    /// * `state` - The new state (closed, open, half_open)
    /// * `is_trip` - Whether this is a trip (closed -> open)
    pub fn record_circuit_breaker_state(upstream: &str, state: &str, is_trip: bool) {
        let labels = CircuitBreakerLabels {
            upstream: upstream.to_string(),
            state: state.to_string(),
        };

        if let Ok(metrics) = METRICS.lock() {
            let state_value = match state {
                "closed" => 0,
                "open" => 1,
                "half_open" => 2,
                _ => 0,
            };
            metrics
                .circuit_breaker_state
                .get_or_create(&labels)
                .set(state_value);

            if is_trip {
                metrics
                    .circuit_breaker_trips_total
                    .get_or_create(&labels)
                    .inc();
            }
        }
    }

    /// Records a rate limit rejection.
    ///
    /// # Arguments
    ///
    /// * `limit_type` - The type of limit (global, per_client)
    pub fn record_rate_limit_rejection(limit_type: &str) {
        let labels = RateLimitLabels {
            limit_type: limit_type.to_string(),
        };

        if let Ok(metrics) = METRICS.lock() {
            metrics
                .rate_limit_rejections_total
                .get_or_create(&labels)
                .inc();
        }
    }

    /// Records a connection event.
    ///
    /// # Arguments
    ///
    /// * `state` - The connection state (accepted, rejected, closed)
    pub fn record_connection(state: &str) {
        let labels = ConnectionLabels {
            state: state.to_string(),
        };

        if let Ok(metrics) = METRICS.lock() {
            metrics.connections_total.get_or_create(&labels).inc();
        }
    }

    /// Sets the number of active connections.
    pub fn set_active_connections(count: i64) {
        if let Ok(metrics) = METRICS.lock() {
            metrics.connections_active.set(count);
        }
    }

    /// Encodes all metrics in Prometheus text format.
    ///
    /// # Errors
    ///
    /// Returns an error if encoding fails or the mutex is poisoned.
    pub fn encode() -> Result<String, io::Error> {
        let metrics = METRICS
            .lock()
            .map_err(|e| io::Error::other(format!("mutex poisoned: {}", e)))?;

        let mut buffer = String::new();
        encode(&mut buffer, &metrics.registry)
            .map_err(|e| io::Error::other(format!("encoding error: {}", e)))?;

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_request() {
        Metrics::record_request("GET", 200, "http://upstream:8080", 0.05);
        Metrics::record_request("POST", 201, "http://upstream:8080", 0.1);

        let encoded = Metrics::encode().unwrap();
        assert!(encoded.contains("http_requests_total"));
        assert!(encoded.contains("http_request_duration_seconds"));
    }

    #[test]
    fn test_record_error() {
        Metrics::record_request("GET", 500, "http://upstream:8080", 0.05);
        Metrics::record_request("GET", 404, "http://upstream:8080", 0.1);

        let encoded = Metrics::encode().unwrap();
        assert!(encoded.contains("http_errors_total"));
    }

    #[test]
    fn test_requests_in_flight() {
        Metrics::inc_requests_in_flight();
        Metrics::inc_requests_in_flight();
        Metrics::dec_requests_in_flight();

        let encoded = Metrics::encode().unwrap();
        assert!(encoded.contains("http_requests_in_flight"));
    }

    #[test]
    fn test_circuit_breaker_metrics() {
        Metrics::record_circuit_breaker_state("http://upstream:8080", "open", true);

        let encoded = Metrics::encode().unwrap();
        assert!(encoded.contains("circuit_breaker_state"));
        assert!(encoded.contains("circuit_breaker_trips_total"));
    }

    #[test]
    fn test_rate_limit_metrics() {
        Metrics::record_rate_limit_rejection("global");
        Metrics::record_rate_limit_rejection("per_client");

        let encoded = Metrics::encode().unwrap();
        assert!(encoded.contains("rate_limit_rejections_total"));
    }

    #[test]
    fn test_connection_metrics() {
        Metrics::record_connection("accepted");
        Metrics::set_active_connections(5);

        let encoded = Metrics::encode().unwrap();
        assert!(encoded.contains("connections_total"));
        assert!(encoded.contains("connections_active"));
    }

    #[test]
    fn test_metrics_encoding() {
        let encoded = Metrics::encode();
        assert!(encoded.is_ok());
    }
}
