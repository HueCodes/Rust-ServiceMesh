//! Protocol negotiation and HTTP/2 support.
//!
//! This module provides ALPN-based protocol negotiation for HTTP/1.1 and HTTP/2
//! connections, with optional TLS support via Rustls.

use crate::error::{ProxyError, Result};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

/// Supported HTTP protocols for the proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpProtocol {
    /// HTTP/1.1 protocol
    Http1,
    /// HTTP/2 protocol
    #[default]
    Http2,
    /// Auto-negotiate based on ALPN (prefers HTTP/2)
    Auto,
}

impl HttpProtocol {
    /// Returns the ALPN protocol identifiers for this protocol.
    pub fn alpn_protocols(&self) -> Vec<Vec<u8>> {
        match self {
            HttpProtocol::Http1 => vec![b"http/1.1".to_vec()],
            HttpProtocol::Http2 => vec![b"h2".to_vec()],
            HttpProtocol::Auto => vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        }
    }

    /// Determines the protocol from ALPN negotiation result.
    pub fn from_alpn(alpn: Option<&[u8]>) -> Self {
        match alpn {
            Some(b"h2") => HttpProtocol::Http2,
            Some(b"http/1.1") => HttpProtocol::Http1,
            _ => HttpProtocol::Http1, // Default to HTTP/1.1 if no ALPN
        }
    }
}

/// TLS configuration for the proxy.
#[derive(Debug, Clone)]
pub struct TlsConfig {
    /// Path to the certificate file (PEM format)
    pub cert_path: String,
    /// Path to the private key file (PEM format)
    pub key_path: String,
    /// Preferred HTTP protocol for negotiation
    pub protocol: HttpProtocol,
}

impl TlsConfig {
    /// Creates a new TLS configuration.
    pub fn new(cert_path: impl Into<String>, key_path: impl Into<String>) -> Self {
        Self {
            cert_path: cert_path.into(),
            key_path: key_path.into(),
            protocol: HttpProtocol::Auto,
        }
    }

    /// Sets the preferred HTTP protocol.
    pub fn with_protocol(mut self, protocol: HttpProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Loads certificates from a PEM file.
    fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
        let file = File::open(path).map_err(|e| ProxyError::TlsConfig {
            message: format!("failed to open cert file: {}", e),
        })?;
        let mut reader = BufReader::new(file);

        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
            .filter_map(|cert| cert.ok())
            .collect();

        if certs.is_empty() {
            return Err(ProxyError::TlsConfig {
                message: "no certificates found in file".to_string(),
            });
        }

        Ok(certs)
    }

    /// Loads a private key from a PEM file.
    fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
        let file = File::open(path).map_err(|e| ProxyError::TlsConfig {
            message: format!("failed to open key file: {}", e),
        })?;
        let mut reader = BufReader::new(file);

        // Try to read PKCS#8 keys first, then RSA keys
        let keys: Vec<PrivateKeyDer<'static>> = rustls_pemfile::read_all(&mut reader)
            .filter_map(|item| match item.ok()? {
                rustls_pemfile::Item::Pkcs1Key(key) => Some(PrivateKeyDer::Pkcs1(key)),
                rustls_pemfile::Item::Pkcs8Key(key) => Some(PrivateKeyDer::Pkcs8(key)),
                rustls_pemfile::Item::Sec1Key(key) => Some(PrivateKeyDer::Sec1(key)),
                _ => None,
            })
            .collect();

        keys.into_iter()
            .next()
            .ok_or_else(|| ProxyError::TlsConfig {
                message: "no private key found in file".to_string(),
            })
    }

    /// Builds a TLS acceptor from this configuration.
    pub fn build_acceptor(&self) -> Result<TlsAcceptor> {
        let certs = Self::load_certs(Path::new(&self.cert_path))?;
        let key = Self::load_private_key(Path::new(&self.key_path))?;

        let mut config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| ProxyError::TlsConfig {
                message: format!("failed to configure TLS: {}", e),
            })?;

        // Configure ALPN protocols
        config.alpn_protocols = self.protocol.alpn_protocols();

        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// Client TLS configuration for connecting to upstream servers.
#[derive(Debug, Clone)]
pub struct ClientTlsConfig {
    /// Whether to verify server certificates
    pub verify_server: bool,
    /// Optional client certificate for mTLS
    pub client_cert: Option<String>,
    /// Optional client key for mTLS
    pub client_key: Option<String>,
}

impl Default for ClientTlsConfig {
    fn default() -> Self {
        Self {
            verify_server: true,
            client_cert: None,
            client_key: None,
        }
    }
}

impl ClientTlsConfig {
    /// Creates a new client TLS configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Disables server certificate verification (not recommended for production).
    pub fn danger_accept_invalid_certs(mut self) -> Self {
        self.verify_server = false;
        self
    }

    /// Sets client certificate for mTLS.
    pub fn with_client_cert(mut self, cert_path: String, key_path: String) -> Self {
        self.client_cert = Some(cert_path);
        self.client_key = Some(key_path);
        self
    }

    /// Builds a Rustls client configuration.
    pub fn build_client_config(&self) -> Result<rustls::ClientConfig> {
        let root_store = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };

        let builder = rustls::ClientConfig::builder().with_root_certificates(root_store);

        let config =
            if let (Some(cert_path), Some(key_path)) = (&self.client_cert, &self.client_key) {
                let certs = TlsConfig::load_certs(Path::new(cert_path))?;
                let key = TlsConfig::load_private_key(Path::new(key_path))?;
                builder
                    .with_client_auth_cert(certs, key)
                    .map_err(|e| ProxyError::TlsConfig {
                        message: format!("failed to configure client auth: {}", e),
                    })?
            } else {
                builder.with_no_client_auth()
            };

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_protocol_alpn() {
        assert_eq!(
            HttpProtocol::Http1.alpn_protocols(),
            vec![b"http/1.1".to_vec()]
        );
        assert_eq!(HttpProtocol::Http2.alpn_protocols(), vec![b"h2".to_vec()]);
        assert_eq!(
            HttpProtocol::Auto.alpn_protocols(),
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn test_protocol_from_alpn() {
        assert_eq!(HttpProtocol::from_alpn(Some(b"h2")), HttpProtocol::Http2);
        assert_eq!(
            HttpProtocol::from_alpn(Some(b"http/1.1")),
            HttpProtocol::Http1
        );
        assert_eq!(HttpProtocol::from_alpn(None), HttpProtocol::Http1);
    }

    #[test]
    fn test_tls_config_builder() {
        let config = TlsConfig::new("cert.pem", "key.pem").with_protocol(HttpProtocol::Http2);

        assert_eq!(config.cert_path, "cert.pem");
        assert_eq!(config.key_path, "key.pem");
        assert_eq!(config.protocol, HttpProtocol::Http2);
    }

    #[test]
    fn test_client_tls_config() {
        let config = ClientTlsConfig::new()
            .danger_accept_invalid_certs()
            .with_client_cert("client.pem".to_string(), "client-key.pem".to_string());

        assert!(!config.verify_server);
        assert_eq!(config.client_cert, Some("client.pem".to_string()));
        assert_eq!(config.client_key, Some("client-key.pem".to_string()));
    }
}
