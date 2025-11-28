# Contributing to Rust Service Mesh

Thank you for your interest in contributing to Rust Service Mesh! This document provides guidelines for contributing to the project.

## Code of Conduct

Be respectful, inclusive, and professional. We're all here to build great software together.

## Getting Started

1. **Fork the repository** on GitHub
2. **Clone your fork** locally:
   ```bash
   git clone https://github.com/YOUR_USERNAME/Rust-ServiceMesh.git
   cd Rust-ServiceMesh
   ```
3. **Create a branch** for your changes:
   ```bash
   git checkout -b feature/my-awesome-feature
   ```

## Development Workflow

### Prerequisites

- Rust 1.75 or later
- Cargo
- Git

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release
```

### Testing

All contributions must include tests and pass existing tests:

```bash
# Run all tests
cargo test

# Run tests for a specific module
cargo test circuit_breaker

# Run with logging
RUST_LOG=debug cargo test

# Run clippy (required)
cargo clippy --all-features -- -D warnings

# Format code (required)
cargo fmt
```

### Code Quality Standards

#### Rust Style
- Follow standard Rust conventions (enforced by `rustfmt`)
- Run `cargo fmt` before committing
- All code must pass `cargo clippy --all-features -- -D warnings`
- Use meaningful variable and function names
- Keep functions under 100 lines when possible

#### Documentation
- Add `///` doc comments to all public items
- Include examples in doc comments for complex APIs
- Update README.md if adding user-facing features
- Doc tests should compile (`cargo test --doc`)

#### Error Handling
- Use `Result` types, avoid panics in library code
- Provide context in error messages
- Use `thiserror` for error types

#### Testing
- Write unit tests for all new functionality
- Add integration tests for end-to-end scenarios
- Aim for >80% code coverage
- Test error paths, not just happy paths

#### Performance
- Profile performance-critical code
- Avoid unnecessary allocations
- Use `Arc` for shared state, avoid `Mutex` when possible
- Prefer lock-free atomics for counters

## Pull Request Process

1. **Ensure your code passes all checks**:
   ```bash
   cargo fmt --check
   cargo clippy --all-features -- -D warnings
   cargo test --all
   cargo build --release
   ```

2. **Update documentation**:
   - Add/update doc comments
   - Update README.md if needed
   - Add examples if introducing new features

3. **Write a clear PR description**:
   - Explain what changes you made and why
   - Reference any related issues
   - Include before/after behavior if applicable

4. **Commit message format**:
   ```
   type: brief description

   Longer explanation if needed.

   Fixes #123
   ```

   Types: `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `chore`

5. **Submit the PR**:
   - Push to your fork
   - Open a PR against `main`
   - Respond to review feedback

## Areas for Contribution

### High Priority
- [ ] Retry logic with exponential backoff
- [ ] Connection pooling in Transport module
- [ ] Rate limiting middleware
- [ ] Health checking for upstreams
- [ ] Additional integration tests

### Medium Priority
- [ ] Distributed tracing (OpenTelemetry)
- [ ] Advanced load balancing algorithms
- [ ] L7 routing implementation
- [ ] HTTP/2 support
- [ ] Benchmarking suite

### Low Priority
- [ ] mTLS support
- [ ] gRPC proxying
- [ ] WASM filter support
- [ ] Kubernetes sidecar mode

## Architecture Guidelines

### Module Organization
- Keep modules focused and single-purpose
- Use `pub(crate)` for internal APIs
- Expose minimal public surface area
- Group related functionality

### Async/Await
- Use Tokio for async runtime
- Avoid blocking operations in async contexts
- Use `tokio::spawn` for CPU-intensive work
- Prefer `tokio::select!` over manual polling

### Dependencies
- Justify new dependencies in your PR
- Prefer well-maintained crates
- Check licenses (Apache-2.0 or MIT compatible)
- Run `cargo audit` to check for vulnerabilities

### Error Handling
```rust
// Good: Contextual errors
.map_err(|e| ProxyError::ListenerBind {
    addr: addr.to_string(),
    source: e,
})?

// Bad: Generic errors
.map_err(|e| format!("Error: {}", e))?
```

### Logging
```rust
// Use tracing macros
use tracing::{debug, info, warn, error, instrument};

#[instrument(level = "debug", skip(self))]
async fn my_function(&self) {
    info!("Starting operation");
    debug!(param = ?value, "Processing");
}
```

## Questions?

- Open an issue for bugs or feature requests
- Start a discussion for design questions
- Check existing issues before creating new ones

## License

By contributing, you agree that your contributions will be dual-licensed under both the MIT License and Apache License 2.0, at the user's option.

---

Thank you for contributing to Rust Service Mesh!
