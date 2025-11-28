mod admin;
mod admin_listener;
mod circuit_breaker;
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

    let proxy_listener = Listener::bind(
        &config.listen_addr,
        config.upstream_addrs_arc(),
        config.request_timeout,
    )
    .await?;
    let proxy_addr = proxy_listener.local_addr();
    info!("proxy listening on {}", proxy_addr);

    let admin_listener = AdminListener::bind(&config.metrics_addr).await?;
    let admin_addr = admin_listener.local_addr();
    info!("admin endpoints on {} (/health, /metrics)", admin_addr);

    let mut proxy_task = tokio::spawn({
        let shutdown_rx = shutdown_tx.subscribe();
        async move {
            if let Err(e) = proxy_listener.serve(shutdown_rx).await {
                error!("proxy listener error: {}", e);
            }
        }
    });

    let mut admin_task = tokio::spawn({
        let shutdown_rx = shutdown_tx.subscribe();
        async move {
            if let Err(e) = admin_listener.serve(shutdown_rx).await {
                error!("admin listener error: {}", e);
            }
        }
    });

    let mut proxy_finished = false;
    let mut admin_finished = false;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("received ctrl-c, initiating graceful shutdown");
        }
        res = &mut proxy_task => {
            proxy_finished = true;
            match res {
                Ok(()) => info!("proxy task completed"),
                Err(err) => error!("proxy task join error: {}", err),
            }
        }
        res = &mut admin_task => {
            admin_finished = true;
            match res {
                Ok(()) => info!("admin task completed"),
                Err(err) => error!("admin task join error: {}", err),
            }
        }
    }

    let _ = shutdown_tx.send(());

    if !proxy_finished {
        match proxy_task.await {
            Ok(()) => info!("proxy task completed"),
            Err(err) => error!("proxy task join error: {}", err),
        }
    }

    if !admin_finished {
        match admin_task.await {
            Ok(()) => info!("admin task completed"),
            Err(err) => error!("admin task join error: {}", err),
        }
    }

    info!("shutdown complete");
    Ok(())
}
