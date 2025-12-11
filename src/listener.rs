//! TCP listener with HTTP/1.1 and HTTP/2 support.
//!
//! This module provides a multi-protocol listener that can handle both HTTP/1.1
//! and HTTP/2 connections, with optional TLS support and ALPN-based protocol
//! negotiation.

use crate::error::{ProxyError, Result};
use crate::protocol::{HttpProtocol, TlsConfig};
use crate::service::ProxyService;
use hyper::body::Incoming;
use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_rustls::TlsAcceptor;
use tower::Service;
use tracing::{debug, error, info, instrument, warn};

/// HTTP listener that accepts connections and spawns handler tasks.
///
/// Supports HTTP/1.1 and HTTP/2 with automatic protocol negotiation via ALPN
/// when TLS is enabled. Without TLS, falls back to HTTP/1.1 or uses prior
/// knowledge for HTTP/2.
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
    tls_acceptor: Option<TlsAcceptor>,
    default_protocol: HttpProtocol,
}

impl Listener {
    /// Binds to the specified address and creates a listener (HTTP only).
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

        info!("bound to {} (HTTP/1.1)", local_addr);

        Ok(Self {
            tcp_listener,
            proxy_service: ProxyService::new(upstream_addrs, request_timeout),
            addr: local_addr,
            tls_acceptor: None,
            default_protocol: HttpProtocol::Http1,
        })
    }

    /// Binds to the specified address with TLS and HTTP/2 support.
    ///
    /// # Arguments
    ///
    /// * `addr` - Address to bind to (e.g., "127.0.0.1:3000")
    /// * `upstream_addrs` - List of upstream server addresses
    /// * `request_timeout` - Maximum duration for upstream requests
    /// * `tls_config` - TLS configuration with certificate and key paths
    ///
    /// # Errors
    ///
    /// Returns `ProxyError::ListenerBind` if binding fails or
    /// `ProxyError::TlsConfig` if TLS configuration is invalid.
    #[instrument(level = "info", skip(upstream_addrs, tls_config))]
    pub async fn bind_with_tls(
        addr: &str,
        upstream_addrs: Arc<Vec<String>>,
        request_timeout: Duration,
        tls_config: TlsConfig,
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

        let tls_acceptor = tls_config.build_acceptor()?;
        let protocol = tls_config.protocol;

        info!("bound to {} (TLS with {:?} support)", local_addr, protocol);

        Ok(Self {
            tcp_listener,
            proxy_service: ProxyService::new(upstream_addrs, request_timeout),
            addr: local_addr,
            tls_acceptor: Some(tls_acceptor),
            default_protocol: protocol,
        })
    }

    /// Binds with HTTP/2 prior knowledge (h2c - HTTP/2 over cleartext).
    ///
    /// This enables HTTP/2 without TLS, using prior knowledge that the
    /// client will speak HTTP/2.
    #[instrument(level = "info", skip(upstream_addrs))]
    pub async fn bind_h2c(
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

        info!("bound to {} (h2c - HTTP/2 cleartext)", local_addr);

        Ok(Self {
            tcp_listener,
            proxy_service: ProxyService::new(upstream_addrs, request_timeout),
            addr: local_addr,
            tls_acceptor: None,
            default_protocol: HttpProtocol::Http2,
        })
    }

    /// Returns the local address the listener is bound to.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns whether TLS is enabled.
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    /// Returns the default HTTP protocol.
    pub fn default_protocol(&self) -> HttpProtocol {
        self.default_protocol
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

        let tls_acceptor = self.tls_acceptor.clone();
        let default_protocol = self.default_protocol;

        loop {
            tokio::select! {
                accept_result = self.tcp_listener.accept() => {
                    match accept_result {
                        Ok((stream, peer_addr)) => {
                            debug!("accepted connection from {}", peer_addr);
                            let service = self.proxy_service.clone();
                            let tls_acceptor = tls_acceptor.clone();

                            tokio::spawn(async move {
                                let result = if let Some(acceptor) = tls_acceptor {
                                    Self::handle_tls_connection(stream, service, acceptor).await
                                } else {
                                    match default_protocol {
                                        HttpProtocol::Http2 => {
                                            Self::handle_h2c_connection(stream, service).await
                                        }
                                        _ => {
                                            Self::handle_http1_connection(stream, service).await
                                        }
                                    }
                                };

                                if let Err(e) = result {
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

    /// Handles a TLS connection with ALPN-based protocol negotiation.
    #[instrument(level = "debug", skip_all)]
    async fn handle_tls_connection(
        stream: tokio::net::TcpStream,
        service: ProxyService,
        acceptor: TlsAcceptor,
    ) -> Result<()> {
        let tls_stream = acceptor
            .accept(stream)
            .await
            .map_err(|e| ProxyError::TlsHandshake(e.to_string()))?;

        // Determine protocol from ALPN negotiation
        let protocol = {
            let (_, server_conn) = tls_stream.get_ref();
            HttpProtocol::from_alpn(server_conn.alpn_protocol())
        };

        debug!("negotiated protocol: {:?}", protocol);

        match protocol {
            HttpProtocol::Http2 => Self::serve_http2(TokioIo::new(tls_stream), service).await,
            _ => Self::serve_http1(TokioIo::new(tls_stream), service).await,
        }
    }

    /// Handles a plain HTTP/1.1 connection.
    #[instrument(level = "debug", skip_all)]
    async fn handle_http1_connection(
        stream: tokio::net::TcpStream,
        service: ProxyService,
    ) -> Result<()> {
        Self::serve_http1(TokioIo::new(stream), service).await
    }

    /// Handles an h2c (HTTP/2 cleartext) connection.
    #[instrument(level = "debug", skip_all)]
    async fn handle_h2c_connection(
        stream: tokio::net::TcpStream,
        service: ProxyService,
    ) -> Result<()> {
        Self::serve_http2(TokioIo::new(stream), service).await
    }

    /// Serves HTTP/1.1 on the given I/O stream.
    async fn serve_http1<I>(io: TokioIo<I>, service: ProxyService) -> Result<()>
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let service = service_fn(move |req: Request<Incoming>| {
            let mut svc = service.clone();
            async move { svc.call(req).await }
        });

        http1::Builder::new()
            .serve_connection(io, service)
            .await
            .map_err(ProxyError::Http)
    }

    /// Serves HTTP/2 on the given I/O stream.
    async fn serve_http2<I>(io: TokioIo<I>, service: ProxyService) -> Result<()>
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let service = service_fn(move |req: Request<Incoming>| {
            let mut svc = service.clone();
            async move { svc.call(req).await }
        });

        http2::Builder::new(TokioExecutor::new())
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
        let listener = listener.unwrap();
        assert!(!listener.is_tls_enabled());
        assert_eq!(listener.default_protocol(), HttpProtocol::Http1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_listener_bind_invalid_address() {
        let upstream = Arc::new(vec!["http://127.0.0.1:9999".to_string()]);
        let timeout = Duration::from_secs(30);
        let listener = Listener::bind("999.999.999.999:0", upstream, timeout).await;
        assert!(listener.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_listener_h2c() {
        let upstream = Arc::new(vec!["http://127.0.0.1:9999".to_string()]);
        let timeout = Duration::from_secs(30);
        let listener = Listener::bind_h2c("127.0.0.1:0", upstream, timeout).await;
        assert!(listener.is_ok());
        let listener = listener.unwrap();
        assert!(!listener.is_tls_enabled());
        assert_eq!(listener.default_protocol(), HttpProtocol::Http2);
    }
}
