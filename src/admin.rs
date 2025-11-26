//! Admin endpoints for health checks and metrics.

use crate::metrics::Metrics;
use http::{Request, Response, StatusCode};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::Service;
use tracing::{debug, warn};

/// Admin service for health checks and metrics endpoints.
///
/// Serves:
/// - `/health` - Health check endpoint returning 200 OK
/// - `/metrics` - Prometheus metrics in text format
///
/// # Example
///
/// ```no_run
/// use rust_servicemesh::admin::AdminService;
///
/// let service = AdminService::new();
/// ```
#[derive(Clone)]
pub struct AdminService;

impl AdminService {
    /// Creates a new admin service.
    pub fn new() -> Self {
        Self
    }

    /// Handles admin requests for health and metrics endpoints.
    async fn handle_request(
        req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>, Infallible> {
        let path = req.uri().path();

        match path {
            "/health" => {
                debug!("health check requested");
                Ok(Self::health_response())
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

    /// Creates a health check response.
    fn health_response() -> Response<BoxBody<Bytes, hyper::Error>> {
        Response::builder()
            .status(StatusCode::OK)
            .body(
                Full::new(Bytes::from("healthy"))
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
        Box::pin(Self::handle_request(req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response() {
        let response = AdminService::health_response();
        assert_eq!(response.status(), StatusCode::OK);
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
