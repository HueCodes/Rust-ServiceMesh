//! Benchmarks for the service mesh proxy.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use rust_servicemesh::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use rust_servicemesh::ratelimit::{RateLimitConfig, RateLimiter};
use rust_servicemesh::retry::{RetryConfig, RetryPolicy};
use rust_servicemesh::router::{PathMatch, Route, Router};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

fn bench_circuit_breaker(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = CircuitBreakerConfig::default();

    c.bench_function("circuit_breaker_allow_request", |b| {
        b.to_async(&rt).iter(|| async {
            let cb = CircuitBreaker::new(config.clone());
            black_box(cb.allow_request().await)
        });
    });

    c.bench_function("circuit_breaker_record_success", |b| {
        b.to_async(&rt).iter(|| async {
            let cb = CircuitBreaker::new(config.clone());
            cb.record_success().await;
            black_box(())
        });
    });

    c.bench_function("circuit_breaker_record_failure", |b| {
        b.to_async(&rt).iter(|| async {
            let cb = CircuitBreaker::new(config.clone());
            cb.record_failure().await;
            black_box(())
        });
    });
}

fn bench_rate_limiter(c: &mut Criterion) {
    let mut group = c.benchmark_group("rate_limiter");
    group.throughput(Throughput::Elements(1));

    let config = RateLimitConfig::new(10000, 1000);
    let limiter = RateLimiter::new(config);
    let client_ip = Some(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)));

    group.bench_function("check_global", |b| {
        b.iter(|| {
            let _ = black_box(limiter.check(None));
        });
    });

    group.bench_function("check_per_client", |b| {
        b.iter(|| {
            let _ = black_box(limiter.check(client_ip));
        });
    });

    group.finish();
}

fn bench_router(c: &mut Criterion) {
    let mut group = c.benchmark_group("router");

    // Build router with various routes
    let mut router = Router::new();
    router.add_route(Route::new(
        "api-users",
        PathMatch::exact("/api/users"),
        "users-cluster",
    ));
    router.add_route(Route::new(
        "api-prefix",
        PathMatch::prefix("/api/"),
        "api-cluster",
    ));
    router.add_route(Route::new(
        "static",
        PathMatch::prefix("/static/"),
        "static-cluster",
    ));
    router.add_route(Route::new(
        "regex-route",
        PathMatch::regex(r"^/v[0-9]+/.*"),
        "versioned-cluster",
    ));

    let headers = http::HeaderMap::new();

    group.bench_function("route_exact_match", |b| {
        b.iter(|| {
            black_box(router.route(&http::Method::GET, "/api/users", &headers));
        });
    });

    group.bench_function("route_prefix_match", |b| {
        b.iter(|| {
            black_box(router.route(&http::Method::GET, "/api/products/123", &headers));
        });
    });

    group.bench_function("route_regex_match", |b| {
        b.iter(|| {
            black_box(router.route(&http::Method::GET, "/v2/resource/abc", &headers));
        });
    });

    group.bench_function("route_no_match", |b| {
        b.iter(|| {
            black_box(router.route(&http::Method::GET, "/unknown/path", &headers));
        });
    });

    group.finish();
}

fn bench_retry_policy(c: &mut Criterion) {
    let mut group = c.benchmark_group("retry");

    group.bench_function("calculate_delay", |b| {
        let config = RetryConfig::new().with_max_retries(5).with_jitter(false);
        let policy = RetryPolicy::new(config);

        b.iter(|| {
            black_box(policy.next_delay());
        });
    });

    group.bench_function("calculate_delay_with_jitter", |b| {
        let config = RetryConfig::new().with_max_retries(5).with_jitter(true);
        let policy = RetryPolicy::new(config);

        b.iter(|| {
            black_box(policy.next_delay());
        });
    });

    group.finish();
}

fn bench_path_matching(c: &mut Criterion) {
    let mut group = c.benchmark_group("path_matching");

    let exact = PathMatch::exact("/api/v1/users");
    let prefix = PathMatch::prefix("/api/");
    let regex = PathMatch::regex(r"^/api/v[0-9]+/users/\d+$");

    group.bench_function("exact_match_hit", |b| {
        b.iter(|| {
            black_box(exact.matches("/api/v1/users"));
        });
    });

    group.bench_function("exact_match_miss", |b| {
        b.iter(|| {
            black_box(exact.matches("/api/v1/products"));
        });
    });

    group.bench_function("prefix_match_hit", |b| {
        b.iter(|| {
            black_box(prefix.matches("/api/v1/users"));
        });
    });

    group.bench_function("prefix_match_miss", |b| {
        b.iter(|| {
            black_box(prefix.matches("/other/path"));
        });
    });

    group.bench_function("regex_match_hit", |b| {
        b.iter(|| {
            black_box(regex.matches("/api/v1/users/123"));
        });
    });

    group.bench_function("regex_match_miss", |b| {
        b.iter(|| {
            black_box(regex.matches("/api/v1/products/abc"));
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_circuit_breaker,
    bench_rate_limiter,
    bench_router,
    bench_retry_policy,
    bench_path_matching,
);

criterion_main!(benches);
