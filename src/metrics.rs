//! Prometheus metrics collection and export.

use once_cell::sync::Lazy;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use std::io;
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

/// Global metrics registry.
///
/// Initialized once at startup and shared across all tasks.
static METRICS: Lazy<Arc<Mutex<Metrics>>> = Lazy::new(|| Arc::new(Mutex::new(Metrics::new())));

/// Metrics collector for the proxy.
///
/// Tracks request counts, latencies, and upstream health.
pub struct Metrics {
    registry: Registry,
    requests_total: Family<HttpLabels, Counter>,
    request_duration_seconds: Family<HttpLabels, Histogram>,
}

impl Metrics {
    /// Creates a new metrics collector with default buckets.
    fn new() -> Self {
        let mut registry = Registry::default();

        let requests_total = Family::<HttpLabels, Counter>::default();
        registry.register(
            "http_requests_total",
            "Total number of HTTP requests",
            requests_total.clone(),
        );

        let request_duration_seconds =
            Family::<HttpLabels, Histogram>::new_with_constructor(|| {
                Histogram::new(exponential_buckets(0.001, 2.0, 10))
            });
        registry.register(
            "http_request_duration_seconds",
            "HTTP request latency in seconds",
            request_duration_seconds.clone(),
        );

        Self {
            registry,
            requests_total,
            request_duration_seconds,
        }
    }

    /// Records an HTTP request with method, status, upstream, and duration.
    ///
    /// # Arguments
    ///
    /// * `method` - HTTP method (e.g., "GET", "POST")
    /// * `status` - HTTP status code (e.g., "200", "404")
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
    fn test_metrics_encoding() {
        let encoded = Metrics::encode();
        assert!(encoded.is_ok());
    }
}
