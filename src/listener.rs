//! TCP listener with graceful shutdown support.

use crate::error::{ProxyError, Result};
use crate::service::ProxyService;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower::Service;
use tracing::{error, info, instrument, warn};

/// HTTP listener that accepts connections and spawns handler tasks.
///
/// Supports graceful shutdown via a broadcast channel.
///
/// # Example
///
/// ```no_run
/// use rust_servicemesh::listener::Listener;
/// use std::sync::Arc;
/// use std::time::Duration;
/// use tokio::sync::broadcast;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let (shutdown_tx, _) = broadcast::channel(1);
///     let upstream = vec!["http://127.0.0.1:8080".to_string()];
///     let timeout = Duration::from_secs(30);
///     let listener = Listener::bind("127.0.0.1:3000", Arc::new(upstream), timeout).await?;
///     listener.serve(shutdown_tx.subscribe()).await?;
///     Ok(())
/// }
/// ```
pub struct Listener {
    tcp_listener: TcpListener,
    proxy_service: ProxyService,
    addr: SocketAddr,
}

impl Listener {
    /// Binds to the specified address and creates a listener.
    ///
    /// # Arguments
    ///
    /// * `addr` - Address to bind to (e.g., "127.0.0.1:3000")
    /// * `upstream_addrs` - List of upstream server addresses
    /// * `request_timeout` - Maximum duration for upstream requests
    ///
    /// # Errors
    ///
    /// Returns `ProxyError::ListenerBind` if binding fails.
    #[instrument(level = "info", skip(upstream_addrs))]
    pub async fn bind(
        addr: &str,
        upstream_addrs: Arc<Vec<String>>,
        request_timeout: Duration,
    ) -> Result<Self> {
        let tcp_listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ProxyError::ListenerBind {
                addr: addr.to_string(),
                source: e,
            })?;

        let local_addr = tcp_listener
            .local_addr()
            .map_err(|e| ProxyError::ListenerBind {
                addr: addr.to_string(),
                source: e,
            })?;

        info!("bound to {}", local_addr);

        Ok(Self {
            tcp_listener,
            proxy_service: ProxyService::new(upstream_addrs, request_timeout),
            addr: local_addr,
        })
    }

    /// Returns the local address the listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Serves incoming connections until a shutdown signal is received.
    ///
    /// Spawns a new task for each connection. Gracefully shuts down when
    /// the shutdown receiver triggers.
    ///
    /// # Arguments
    ///
    /// * `shutdown_rx` - Broadcast receiver for shutdown signal
    #[instrument(level = "info", skip(self, shutdown_rx), fields(addr = %self.addr))]
    pub async fn serve(self, mut shutdown_rx: broadcast::Receiver<()>) -> Result<()> {
        info!("serving connections");

        loop {
            tokio::select! {
                accept_result = self.tcp_listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            info!("accepted connection from {}", peer_addr);
                            let service = self.proxy_service.clone();
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(stream, service).await {
                                    error!("connection error from {}: {}", peer_addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            warn!("failed to accept connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("received shutdown signal, stopping listener");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handles a single TCP connection using HTTP/1.1.
    #[instrument(level = "debug", skip(stream, service))]
    async fn handle_connection(stream: tokio::net::TcpStream, service: ProxyService) -> Result<()> {
        let io = TokioIo::new(stream);

        let service = service_fn(move |req: Request<Incoming>| {
            let mut service = service.clone();
            async move { service.call(req).await }
        });

        http1::Builder::new()
            .serve_connection(io, service)
            .await
            .map_err(ProxyError::Http)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_listener_bind() {
        let upstream = Arc::new(vec!["http://127.0.0.1:9999".to_string()]);
        let timeout = Duration::from_secs(30);
        let listener = Listener::bind("127.0.0.1:0", upstream, timeout).await;
        assert!(listener.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_listener_bind_invalid_address() {
        let upstream = Arc::new(vec!["http://127.0.0.1:9999".to_string()]);
        let timeout = Duration::from_secs(30);
        let listener = Listener::bind("999.999.999.999:0", upstream, timeout).await;
        assert!(listener.is_err());
    }
}
