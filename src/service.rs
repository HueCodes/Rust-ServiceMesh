//! Tower service implementation for HTTP proxying.

use crate::error::{ProxyError, Result};
use crate::metrics::Metrics;
use http::{Request, Response, StatusCode, Uri};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tower::Service;
use tracing::{debug, info, instrument, warn};

/// HTTP proxy service that forwards requests to upstream servers.
///
/// Implements `tower::Service` for composability with Tower middleware.
///
/// # Example
///
/// ```no_run
/// use rust_servicemesh::service::ProxyService;
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// #[tokio::main]
/// async fn main() {
///     let upstream = "http://example.com:8080".to_string();
///     let timeout = Duration::from_secs(30);
///     let service = ProxyService::new(Arc::new(vec![upstream]), timeout);
/// }
/// ```
#[derive(Clone)]
pub struct ProxyService {
    upstream_addrs: Arc<Vec<String>>,
    client: Client<HttpConnector, Incoming>,
    next_upstream: Arc<std::sync::atomic::AtomicUsize>,
    request_timeout: Duration,
}

impl ProxyService {
    /// Creates a new proxy service with the given upstream addresses.
    ///
    /// # Arguments
    ///
    /// * `upstream_addrs` - List of upstream server addresses (e.g., "http://127.0.0.1:8080")
    /// * `request_timeout` - Maximum duration for upstream requests
    pub fn new(upstream_addrs: Arc<Vec<String>>, request_timeout: Duration) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http();
        Self {
            upstream_addrs,
            client,
            next_upstream: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            request_timeout,
        }
    }

    /// Selects the next upstream server using round-robin load balancing.
    #[instrument(level = "debug", skip(self))]
    fn select_upstream(&self) -> Result<&str> {
        if self.upstream_addrs.is_empty() {
            return Err(ProxyError::NoUpstream);
        }

        let idx = self
            .next_upstream
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.upstream_addrs.len();

        Ok(&self.upstream_addrs[idx])
    }

    /// Forwards an HTTP request to the selected upstream server.
    #[instrument(level = "debug", skip(self, req), fields(method = %req.method(), uri = %req.uri()))]
    async fn forward_request(
        &self,
        mut req: Request<Incoming>,
    ) -> Result<Response<BoxBody<Bytes, hyper::Error>>> {
        let start = Instant::now();
        let method = req.method().to_string();

        let upstream = self.select_upstream()?;
        let upstream_owned = upstream.to_string();

        let upstream_uri = match self.build_upstream_uri(upstream, req.uri()) {
            Ok(uri) => uri,
            Err(e) => {
                warn!("failed to build upstream URI: {}", e);
                let duration = start.elapsed().as_secs_f64();
                Metrics::record_request(&method, 502, &upstream_owned, duration);
                return Ok(Self::error_response(
                    StatusCode::BAD_GATEWAY,
                    "Invalid upstream URI",
                ));
            }
        };

        debug!("forwarding to upstream: {}", upstream_uri);
        *req.uri_mut() = upstream_uri;

        match timeout(self.request_timeout, self.client.request(req)).await {
            Ok(Ok(response)) => {
                let status = response.status().as_u16();
                let duration = start.elapsed().as_secs_f64();

                info!(
                    method = %method,
                    status = status,
                    upstream = %upstream_owned,
                    duration_ms = duration * 1000.0,
                    "request completed"
                );

                Metrics::record_request(&method, status, &upstream_owned, duration);

                let (parts, body) = response.into_parts();
                let boxed_body = body.boxed();
                Ok(Response::from_parts(parts, boxed_body))
            }
            Ok(Err(e)) => {
                warn!("upstream request failed: {}", e);
                let duration = start.elapsed().as_secs_f64();
                Metrics::record_request(&method, 502, &upstream_owned, duration);
                Ok(Self::error_response(
                    StatusCode::BAD_GATEWAY,
                    "Upstream request failed",
                ))
            }
            Err(_) => {
                warn!("upstream request timed out after {:?}", self.request_timeout);
                let duration = start.elapsed().as_secs_f64();
                Metrics::record_request(&method, 504, &upstream_owned, duration);
                Ok(Self::error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "Upstream request timed out",
                ))
            }
        }
    }

    /// Builds the full upstream URI from the upstream address and request path.
    fn build_upstream_uri(&self, upstream: &str, original_uri: &Uri) -> Result<Uri> {
        let path_and_query = original_uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        let uri_str = format!("{}{}", upstream, path_and_query);
        uri_str
            .parse()
            .map_err(|e| ProxyError::ServiceUnavailable(format!("failed to parse URI: {}", e)))
    }

    /// Creates an HTTP error response.
    fn error_response(status: StatusCode, message: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
        let body = Full::new(Bytes::from(message.to_string()))
            .map_err(|never| match never {})
            .boxed();
        Response::builder()
            .status(status)
            .body(body)
            .unwrap_or_else(|_| Response::new(Empty::new().map_err(|never| match never {}).boxed()))
    }
}

impl Service<Request<Incoming>> for ProxyService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[instrument(level = "debug", skip(self, req), fields(method = %req.method(), uri = %req.uri()))]
    fn call(&mut self, req: Request<Incoming>) -> Self::Future {
        let this = self.clone();
        Box::pin(async move {
            let response = match this.forward_request(req).await {
                Ok(resp) => resp,
                Err(e) => {
                    warn!("proxy error: {}", e);
                    Self::error_response(StatusCode::BAD_GATEWAY, "Proxy error")
                }
            };
            Ok(response)
        })
    }
}
