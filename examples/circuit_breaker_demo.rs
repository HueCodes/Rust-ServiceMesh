//! Circuit breaker demonstration.
//!
//! Shows how the circuit breaker transitions between states based on failures and successes.
//!
//! Run with:
//! ```bash
//! cargo run --example circuit_breaker_demo
//! ```

use rust_servicemesh::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, State};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .init();

    info!("Circuit Breaker Demonstration");
    info!("==============================\n");

    // Configure circuit breaker
    let config = CircuitBreakerConfig {
        failure_threshold: 3,
        timeout: Duration::from_secs(2),
        success_threshold: 2,
    };

    info!("Configuration:");
    info!("  Failure threshold: {}", config.failure_threshold);
    info!("  Timeout: {:?}", config.timeout);
    info!("  Success threshold: {}\n", config.success_threshold);

    let cb = CircuitBreaker::new(config);

    // Scenario 1: Closed -> Open (failures)
    info!("Scenario 1: Triggering circuit breaker with failures");
    info!("State: {:?}", cb.state().await);

    for i in 1..=3 {
        if cb.allow_request().await {
            info!("  Request #{} allowed", i);
            simulate_request(false).await;
            cb.record_failure().await;
            info!("  Recorded failure");
        }
    }

    info!("State: {:?}\n", cb.state().await);
    assert_eq!(cb.state().await, State::Open);

    // Scenario 2: Open -> reject requests
    info!("Scenario 2: Requests rejected while circuit is open");
    if cb.allow_request().await {
        info!("  Request allowed (unexpected!)");
    } else {
        warn!("  Request REJECTED - circuit is open");
    }
    info!("State: {:?}\n", cb.state().await);

    // Scenario 3: Open -> HalfOpen (timeout)
    info!("Scenario 3: Waiting for timeout to transition to HalfOpen");
    info!("  Sleeping for {:?}...", Duration::from_secs(2));
    sleep(Duration::from_secs(2)).await;

    if cb.allow_request().await {
        info!("  Request allowed - circuit is now HalfOpen");
    }
    info!("State: {:?}\n", cb.state().await);
    assert_eq!(cb.state().await, State::HalfOpen);

    // Scenario 4: HalfOpen -> Closed (successes)
    info!("Scenario 4: Recording successes to close the circuit");
    for i in 1..=2 {
        if cb.allow_request().await {
            info!("  Request #{} allowed", i);
            simulate_request(true).await;
            cb.record_success().await;
            info!("  Recorded success");
        }
    }

    info!("State: {:?}\n", cb.state().await);
    assert_eq!(cb.state().await, State::Closed);

    // Scenario 5: HalfOpen -> Open (failure)
    info!("Scenario 5: HalfOpen failure reopens circuit immediately");
    cb.reset().await;

    // Trigger open
    for _ in 0..3 {
        cb.allow_request().await;
        cb.record_failure().await;
    }

    sleep(Duration::from_secs(2)).await;
    cb.allow_request().await; // Transition to HalfOpen

    info!("State before failure: {:?}", cb.state().await);
    cb.record_failure().await;
    info!("State after failure: {:?}\n", cb.state().await);
    assert_eq!(cb.state().await, State::Open);

    // Statistics
    info!("Final Statistics:");
    let stats = cb.stats();
    info!("  Total requests: {}", stats.total_requests);
    info!("  Total failures: {}", stats.total_failures);
    info!(
        "  Failure rate: {:.1}%",
        (stats.total_failures as f64 / stats.total_requests as f64) * 100.0
    );

    info!("\nDemo complete!");
}

/// Simulates a request with configurable success/failure.
async fn simulate_request(success: bool) {
    sleep(Duration::from_millis(10)).await;
    if success {
        info!("    [Simulated request succeeded]");
    } else {
        warn!("    [Simulated request failed]");
    }
}
