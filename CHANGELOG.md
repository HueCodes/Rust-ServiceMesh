# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- HTTP/2 support with ALPN-based protocol negotiation
- TLS termination using Rustls with modern cipher suites
- h2c (HTTP/2 over cleartext) support for internal traffic
- L7 routing with path, header, and method-based matching
- Regex support for path matching in routes
- Path rewriting capabilities in routing rules
- Token bucket rate limiting with per-client and global limits
- Retry logic with exponential backoff and jitter
- Connection pooling and load balancing transport layer
- Multiple load balancing strategies (round-robin, least connections, random)
- Endpoint health tracking with circuit breaker integration
- Comprehensive benchmark suite using Criterion

### Changed
- Refactored listener to support multiple protocols
- Enhanced error types with additional variants for new features
- Improved configuration system with builder patterns
- Updated dependencies to latest versions

### Fixed
- Proper graceful shutdown handling for all connection types

## [0.1.0] - Initial Release

### Added
- Async HTTP/1.1 proxy using Tokio, Hyper, and Tower
- Round-robin load balancing
- Circuit breaker with Hystrix-style state machine
- Prometheus metrics integration
- Admin endpoints for health checks and metrics
- Graceful shutdown support
- Basic integration tests
- GitHub Actions CI pipeline
