mod config;
mod error;
mod listener;
mod metrics;
mod router;
mod service;
mod transport;

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
    let config = ProxyConfig::default();
    info!(
        "config: listen={}, upstream={:?}",
        config.listen_addr, config.upstream_addrs
    );

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

    let listen_addr = config.listen_addr.clone();
    let listener = Listener::bind(&listen_addr, config.into_arc()).await?;
    let listen_addr = listener.local_addr();
    info!("proxy listening on {}", listen_addr);

    let serve_task = tokio::spawn(async move {
        if let Err(e) = listener.serve(shutdown_rx).await {
            error!("listener error: {}", e);
        }
    });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, initiating graceful shutdown");
            let _ = shutdown_tx.send(());
        }
        _ = serve_task => {
            info!("listener task completed");
        }
    }

    info!("shutdown complete");
    Ok(())
}
