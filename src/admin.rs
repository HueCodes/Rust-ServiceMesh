//! Admin endpoints for health checks, readiness, and metrics.

use crate::metrics::Metrics;
use http::{Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use serde::Serialize;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tower::Service;
use tracing::{debug, warn};

/// Readiness state that can be shared across the application.
#[derive(Debug)]
pub struct ReadinessState {
    ready: AtomicBool,
}

impl ReadinessState {
    /// Creates a new readiness state, initially not ready.
    pub fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
        }
    }

    /// Sets the readiness state to ready.
    pub fn set_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
    }

    /// Sets the readiness state to not ready.
    pub fn set_not_ready(&self) {
        self.ready.store(false, Ordering::SeqCst);
    }

    /// Checks if the service is ready.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
}

impl Default for ReadinessState {
    fn default() -> Self {
        Self::new()
    }
}

/// Health check response format.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// Readiness check response format.
#[derive(Debug, Serialize)]
struct ReadinessResponse {
    ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// Admin service for health checks and metrics endpoints.
///
/// Serves:
/// - `/health` - Liveness check endpoint returning 200 OK
/// - `/ready` - Readiness check endpoint (200 if ready, 503 if not)
/// - `/metrics` - Prometheus metrics in text format
///
/// # Example
///
/// ```no_run
/// use rust_servicemesh::admin::AdminService;
/// use std::sync::Arc;
///
/// let service = AdminService::new();
/// // Or with custom readiness state:
/// // let service = AdminService::with_readiness(Arc::new(ReadinessState::new()));
/// ```
#[derive(Clone)]
pub struct AdminService {
    readiness: Arc<ReadinessState>,
}

impl AdminService {
    /// Creates a new admin service with default readiness (starts as ready).
    pub fn new() -> Self {
        let readiness = Arc::new(ReadinessState::new());
        readiness.set_ready(); // Default to ready for backwards compatibility
        Self { readiness }
    }

    /// Creates an admin service with custom readiness state.
    pub fn with_readiness(readiness: Arc<ReadinessState>) -> Self {
        Self { readiness }
    }

    /// Returns the readiness state reference.
    pub fn readiness(&self) -> &Arc<ReadinessState> {
        &self.readiness
    }

    /// Handles admin requests for health, readiness, and metrics endpoints.
    async fn handle_request(
        readiness: Arc<ReadinessState>,
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, Infallible> {
        let path = req.uri().path();

        match path {
            "/health" | "/healthz" => {
                debug!("health check requested");
                Ok(Self::health_response())
            }
            "/ready" | "/readyz" => {
                debug!("readiness check requested");
                Ok(Self::readiness_response(&readiness))
            }
            "/metrics" => {
                debug!("metrics requested");
                match Metrics::encode() {
                    Ok(metrics) => Ok(Self::metrics_response(metrics)),
                    Err(e) => {
                        warn!("failed to encode metrics: {}", e);
                        Ok(Self::error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "Failed to encode metrics",
                        ))
                    }
                }
            }
            _ => Ok(Self::error_response(StatusCode::NOT_FOUND, "Not Found")),
        }
    }

    /// Creates a health check response (liveness probe).
    fn health_response() -> Response<BoxBody<Bytes, hyper::Error>> {
        let response = HealthResponse { status: "healthy" };
        let body = serde_json::to_string(&response)
            .unwrap_or_else(|_| r#"{"status":"healthy"}"#.to_string());

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(
                Full::new(Bytes::from(body))
                    .map_err(|never| match never {})
                    .boxed(),
            )
            .unwrap_or_else(|_| {
                Response::new(
                    Full::new(Bytes::new())
                        .map_err(|never| match never {})
                        .boxed(),
                )
            })
    }

    /// Creates a readiness check response.
    fn readiness_response(readiness: &ReadinessState) -> Response<BoxBody<Bytes, hyper::Error>> {
        let is_ready = readiness.is_ready();
        let response = ReadinessResponse {
            ready: is_ready,
            reason: if is_ready {
                None
            } else {
                Some("service not ready".to_string())
            },
        };
        let body = serde_json::to_string(&response)
            .unwrap_or_else(|_| format!(r#"{{"ready":{}}}"#, is_ready));

        let status = if is_ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        };

        Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(
                Full::new(Bytes::from(body))
                    .map_err(|never| match never {})
                    .boxed(),
            )
            .unwrap_or_else(|_| {
                Response::new(
                    Full::new(Bytes::new())
                        .map_err(|never| match never {})
                        .boxed(),
                )
            })
    }

    /// Creates a metrics response in Prometheus text format.
    fn metrics_response(metrics: String) -> Response<BoxBody<Bytes, hyper::Error>> {
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/plain; version=0.0.4")
            .body(
                Full::new(Bytes::from(metrics))
                    .map_err(|never| match never {})
                    .boxed(),
            )
            .unwrap_or_else(|_| {
                Response::new(
                    Full::new(Bytes::new())
                        .map_err(|never| match never {})
                        .boxed(),
                )
            })
    }

    /// Creates an HTTP error response.
    fn error_response(status: StatusCode, message: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
        Response::builder()
            .status(status)
            .body(
                Full::new(Bytes::from(message.to_string()))
                    .map_err(|never| match never {})
                    .boxed(),
            )
            .unwrap_or_else(|_| {
                Response::new(
                    Full::new(Bytes::new())
                        .map_err(|never| match never {})
                        .boxed(),
                )
            })
    }
}

impl Default for AdminService {
    fn default() -> Self {
        Self::new()
    }
}

impl Service<Request<Incoming>> for AdminService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let readiness = Arc::clone(&self.readiness);
        Box::pin(Self::handle_request(readiness, req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response() {
        let response = AdminService::health_response();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "application/json"
        );
    }

    #[test]
    fn test_readiness_response_ready() {
        let state = ReadinessState::new();
        state.set_ready();
        let response = AdminService::readiness_response(&state);
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn test_readiness_response_not_ready() {
        let state = ReadinessState::new();
        let response = AdminService::readiness_response(&state);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_readiness_state() {
        let state = ReadinessState::new();
        assert!(!state.is_ready());

        state.set_ready();
        assert!(state.is_ready());

        state.set_not_ready();
        assert!(!state.is_ready());
    }

    #[test]
    fn test_metrics_response() {
        let metrics = "test_metric 1.0".to_string();
        let response = AdminService::metrics_response(metrics);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "text/plain; version=0.0.4"
        );
    }

    #[test]
    fn test_error_response() {
        let response = AdminService::error_response(StatusCode::NOT_FOUND, "Not Found");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
