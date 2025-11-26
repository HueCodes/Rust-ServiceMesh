//! Admin listener for health checks and metrics.

use crate::admin::AdminService;
use crate::error::{ProxyError, Result};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tower::Service;
use tracing::{error, info, instrument, warn};

/// Admin HTTP listener for health and metrics endpoints.
///
/// Serves admin endpoints on a separate port for monitoring and observability.
///
/// # Example
///
/// ```no_run
/// use rust_servicemesh::admin_listener::AdminListener;
/// use tokio::sync::broadcast;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let (shutdown_tx, _) = broadcast::channel(1);
///     let listener = AdminListener::bind("127.0.0.1:9090").await?;
///     listener.serve(shutdown_tx.subscribe()).await?;
///     Ok(())
/// }
/// ```
pub struct AdminListener {
    tcp_listener: TcpListener,
    admin_service: AdminService,
    addr: SocketAddr,
}

impl AdminListener {
    /// Binds to the specified address for admin endpoints.
    ///
    /// # Arguments
    ///
    /// * `addr` - Address to bind to (e.g., "127.0.0.1:9090")
    ///
    /// # Errors
    ///
    /// Returns `ProxyError::ListenerBind` if binding fails.
    #[instrument(level = "info")]
    pub async fn bind(addr: &str) -> Result<Self> {
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

        info!("admin endpoint bound to {}", local_addr);

        Ok(Self {
            tcp_listener,
            admin_service: AdminService::new(),
            addr: local_addr,
        })
    }

    /// Returns the local address the listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Serves admin endpoints until a shutdown signal is received.
    ///
    /// # Arguments
    ///
    /// * `shutdown_rx` - Broadcast receiver for shutdown signal
    #[instrument(level = "info", skip(self, shutdown_rx), fields(addr = %self.addr))]
    pub async fn serve(self, mut shutdown_rx: broadcast::Receiver<()>) -> Result<()> {
        info!("serving admin endpoints");

        loop {
            tokio::select! {
                accept_result = self.tcp_listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            info!("admin connection from {}", peer_addr);
                            let service = self.admin_service.clone();
                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_connection(stream, service).await {
                                    error!("admin connection error from {}: {}", peer_addr, e);
                                }
                            });
                        }
                        Err(e) => {
                            warn!("failed to accept admin connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("received shutdown signal, stopping admin listener");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handles a single admin TCP connection.
    #[instrument(level = "debug", skip(stream, service))]
    async fn handle_connection(stream: tokio::net::TcpStream, service: AdminService) -> Result<()> {
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
    async fn test_admin_listener_bind() {
        let listener = AdminListener::bind("127.0.0.1:0").await;
        assert!(listener.is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_admin_listener_bind_invalid_address() {
        let listener = AdminListener::bind("999.999.999.999:0").await;
        assert!(listener.is_err());
    }
}
