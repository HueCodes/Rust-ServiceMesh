//! Rust Service Mesh - High-performance data plane proxy
//!
//! A service mesh proxy built with Rust, inspired by Envoy, providing
//! HTTP/1.1 and HTTP/2 proxying, load balancing, and observability.

pub mod config;
pub mod error;
pub mod listener;
pub mod metrics;
pub mod router;
pub mod service;
pub mod transport;
