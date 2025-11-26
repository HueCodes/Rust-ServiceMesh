mod admin;
mod admin_listener;
mod config;
mod error;
mod listener;
mod metrics;
mod router;
mod service;
mod transport;

use admin_listener::AdminListener;
use config::ProxyConfig;
use listener::Listener;
use tokio::sync::broadcast;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting Rust Service Mesh Proxy");

    if let Err(e) = run().await {
        error!("fatal error: {}", e);
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = ProxyConfig::from_env();
    info!(
        "config: proxy={}, admin={}, upstream={:?}, timeout={}ms",
        config.listen_addr,
        config.metrics_addr,
        config.upstream_addrs,
        config.request_timeout.as_millis()
    );

    let (shutdown_tx, _shutdown_rx) = broadcast::channel(1);

    let proxy_listener = Listener::bind(&config.listen_addr, config.upstream_addrs_arc()).await?;
    let proxy_addr = proxy_listener.local_addr();
    info!("proxy listening on {}", proxy_addr);

    let admin_listener = AdminListener::bind(&config.metrics_addr).await?;
    let admin_addr = admin_listener.local_addr();
    info!("admin endpoints on {} (/health, /metrics)", admin_addr);

    let proxy_task = tokio::spawn({
        let shutdown_rx = shutdown_tx.subscribe();
        async move {
            if let Err(e) = proxy_listener.serve(shutdown_rx).await {
                error!("proxy listener error: {}", e);
            }
        }
    });

    let admin_task = tokio::spawn({
        let shutdown_rx = shutdown_tx.subscribe();
        async move {
            if let Err(e) = admin_listener.serve(shutdown_rx).await {
                error!("admin listener error: {}", e);
            }
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, initiating graceful shutdown");
            let _ = shutdown_tx.send(());
        }
        _ = proxy_task => {
            info!("proxy task completed");
        }
        _ = admin_task => {
            info!("admin task completed");
        }
    }

    info!("shutdown complete");
    Ok(())
}
