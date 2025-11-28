//! Basic proxy example demonstrating minimal setup.
//!
//! Run with:
//! ```bash
//! cargo run --example basic_proxy
//! ```

use rust_servicemesh::listener::Listener;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting basic proxy example");

    // Configure upstream servers
    let upstream_addrs = Arc::new(vec![
        "http://httpbin.org".to_string(),
    ]);

    // Configure request timeout
    let timeout = Duration::from_secs(30);

    // Create listener
    let listener = match Listener::bind("127.0.0.1:3000", upstream_addrs, timeout).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind listener: {}", e);
            return;
        }
    };

    let addr = listener.local_addr();
    info!("Proxy listening on http://{}", addr);
    info!("Try: curl http://{}/get", addr);

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    // Spawn proxy server
    tokio::spawn(async move {
        if let Err(e) = listener.serve(shutdown_rx).await {
            error!("Listener error: {}", e);
        }
    });

    // Wait for Ctrl+C
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            info!("Received Ctrl+C, shutting down");
            let _ = shutdown_tx.send(());
        }
        Err(e) => {
            error!("Failed to listen for Ctrl+C: {}", e);
        }
    }

    // Give tasks time to clean up
    tokio::time::sleep(Duration::from_millis(100)).await;
    info!("Shutdown complete");
}
