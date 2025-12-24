//! L7 routing with path and header-based matching.
//!
//! Provides flexible routing rules for directing traffic to different
//! upstream clusters based on request attributes.

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, warn};

/// Global regex cache to avoid recompiling patterns on every request.
static REGEX_CACHE: Lazy<RwLock<HashMap<String, Arc<Regex>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Gets or compiles a regex pattern, caching the result.
fn get_or_compile_regex(pattern: &str) -> Option<Arc<Regex>> {
    // Fast path: check if already cached
    {
        let cache = REGEX_CACHE.read();
        if let Some(regex) = cache.get(pattern) {
            return Some(Arc::clone(regex));
        }
    }

    // Slow path: compile and cache
    match Regex::new(pattern) {
        Ok(regex) => {
            let regex = Arc::new(regex);
            let mut cache = REGEX_CACHE.write();
            cache.insert(pattern.to_string(), Arc::clone(&regex));
            Some(regex)
        }
        Err(e) => {
            warn!(pattern = %pattern, error = %e, "invalid regex pattern");
            None
        }
    }
}

/// Route matching priority (higher = evaluated first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoutePriority {
    /// Exact match routes (highest priority).
    Exact = 100,
    /// Prefix match routes.
    Prefix = 50,
    /// Regex match routes.
    Regex = 25,
    /// Default/catch-all routes (lowest priority).
    Default = 0,
}

/// Condition for matching a header.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HeaderMatch {
    /// Header must have exactly this value.
    Exact { name: String, value: String },
    /// Header must contain this substring.
    Contains { name: String, value: String },
    /// Header must match this regex pattern.
    Regex { name: String, pattern: String },
    /// Header must be present (any value).
    Present { name: String },
    /// Header must be absent.
    Absent { name: String },
}

impl HeaderMatch {
    /// Creates an exact header match.
    pub fn exact(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Exact {
            name: name.into(),
            value: value.into(),
        }
    }

    /// Creates a header presence check.
    pub fn present(name: impl Into<String>) -> Self {
        Self::Present { name: name.into() }
    }

    /// Creates a header absence check.
    pub fn absent(name: impl Into<String>) -> Self {
        Self::Absent { name: name.into() }
    }

    /// Checks if the header matches.
    pub fn matches(&self, headers: &http::HeaderMap) -> bool {
        match self {
            HeaderMatch::Exact { name, value } => {
                headers.get(name).is_some_and(|v| v == value.as_str())
            }
            HeaderMatch::Contains { name, value } => headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains(value.as_str())),
            HeaderMatch::Regex { name, pattern } => {
                if let Some(regex) = get_or_compile_regex(pattern) {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| regex.is_match(v))
                } else {
                    false
                }
            }
            HeaderMatch::Present { name } => headers.contains_key(name),
            HeaderMatch::Absent { name } => !headers.contains_key(name),
        }
    }
}

/// Condition for matching a request path.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PathMatch {
    /// Path must be exactly this value.
    Exact { path: String },
    /// Path must start with this prefix.
    Prefix { prefix: String },
    /// Path must match this regex pattern.
    Regex { pattern: String },
}

impl PathMatch {
    /// Creates an exact path match.
    pub fn exact(path: impl Into<String>) -> Self {
        Self::Exact { path: path.into() }
    }

    /// Creates a prefix path match.
    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self::Prefix {
            prefix: prefix.into(),
        }
    }

    /// Creates a regex path match.
    pub fn regex(pattern: impl Into<String>) -> Self {
        Self::Regex {
            pattern: pattern.into(),
        }
    }

    /// Checks if the path matches.
    pub fn matches(&self, path: &str) -> bool {
        match self {
            PathMatch::Exact { path: expected } => path == expected,
            PathMatch::Prefix { prefix } => path.starts_with(prefix),
            PathMatch::Regex { pattern } => {
                if let Some(regex) = get_or_compile_regex(pattern) {
                    regex.is_match(path)
                } else {
                    false
                }
            }
        }
    }

    /// Returns the priority for this match type.
    pub fn priority(&self) -> RoutePriority {
        match self {
            PathMatch::Exact { .. } => RoutePriority::Exact,
            PathMatch::Prefix { .. } => RoutePriority::Prefix,
            PathMatch::Regex { .. } => RoutePriority::Regex,
        }
    }
}

/// Condition for matching HTTP method.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum MethodMatch {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
    #[default]
    Any,
}

impl MethodMatch {
    /// Checks if the method matches.
    pub fn matches(&self, method: &http::Method) -> bool {
        match self {
            MethodMatch::Any => true,
            MethodMatch::Get => method == http::Method::GET,
            MethodMatch::Post => method == http::Method::POST,
            MethodMatch::Put => method == http::Method::PUT,
            MethodMatch::Delete => method == http::Method::DELETE,
            MethodMatch::Patch => method == http::Method::PATCH,
            MethodMatch::Head => method == http::Method::HEAD,
            MethodMatch::Options => method == http::Method::OPTIONS,
        }
    }
}

/// A single routing rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    /// Unique name for this route.
    pub name: String,
    /// Path matching condition.
    pub path: PathMatch,
    /// HTTP method matching (optional, defaults to Any).
    #[serde(default)]
    pub method: MethodMatch,
    /// Header matching conditions (all must match).
    #[serde(default)]
    pub headers: Vec<HeaderMatch>,
    /// Target upstream cluster name.
    pub upstream: String,
    /// Weight for load balancing (when multiple routes match).
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// Whether this route is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Request timeout override for this route.
    pub timeout_ms: Option<u64>,
    /// Path rewrite (replace matched path with this).
    pub rewrite: Option<String>,
}

fn default_weight() -> u32 {
    100
}

fn default_enabled() -> bool {
    true
}

impl Route {
    /// Creates a new route with the given name and path.
    pub fn new(name: impl Into<String>, path: PathMatch, upstream: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            path,
            method: MethodMatch::Any,
            headers: Vec::new(),
            upstream: upstream.into(),
            weight: 100,
            enabled: true,
            timeout_ms: None,
            rewrite: None,
        }
    }

    /// Sets the HTTP method for this route.
    pub fn with_method(mut self, method: MethodMatch) -> Self {
        self.method = method;
        self
    }

    /// Adds a header match condition.
    pub fn with_header(mut self, header: HeaderMatch) -> Self {
        self.headers.push(header);
        self
    }

    /// Sets the weight for this route.
    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    /// Sets a timeout override.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Sets a path rewrite rule.
    pub fn with_rewrite(mut self, rewrite: impl Into<String>) -> Self {
        self.rewrite = Some(rewrite.into());
        self
    }

    /// Checks if this route matches the request.
    pub fn matches(&self, method: &http::Method, path: &str, headers: &http::HeaderMap) -> bool {
        if !self.enabled {
            return false;
        }

        if !self.method.matches(method) {
            return false;
        }

        if !self.path.matches(path) {
            return false;
        }

        for header_match in &self.headers {
            if !header_match.matches(headers) {
                return false;
            }
        }

        true
    }

    /// Returns the priority of this route.
    pub fn priority(&self) -> RoutePriority {
        self.path.priority()
    }
}

/// Result of a route match.
#[derive(Debug, Clone)]
pub struct RouteMatch {
    /// The matched route.
    pub route: Route,
    /// Rewritten path (if applicable).
    pub rewritten_path: Option<String>,
}

/// Router for L7 traffic routing.
pub struct Router {
    routes: Vec<Route>,
    default_upstream: Option<String>,
}

impl Router {
    /// Creates a new router with no routes.
    pub fn new() -> Self {
        Self {
            routes: Vec::new(),
            default_upstream: None,
        }
    }

    /// Creates a router with the given routes.
    pub fn with_routes(routes: Vec<Route>) -> Self {
        let mut router = Self {
            routes,
            default_upstream: None,
        };
        router.sort_routes();
        router
    }

    /// Sets the default upstream for unmatched routes.
    pub fn with_default_upstream(mut self, upstream: impl Into<String>) -> Self {
        self.default_upstream = Some(upstream.into());
        self
    }

    /// Adds a route to the router.
    pub fn add_route(&mut self, route: Route) {
        self.routes.push(route);
        self.sort_routes();
    }

    /// Removes a route by name.
    pub fn remove_route(&mut self, name: &str) -> Option<Route> {
        if let Some(pos) = self.routes.iter().position(|r| r.name == name) {
            Some(self.routes.remove(pos))
        } else {
            None
        }
    }

    /// Sorts routes by priority (highest first).
    fn sort_routes(&mut self) {
        self.routes.sort_by_key(|r| std::cmp::Reverse(r.priority()));
    }

    /// Finds the matching route for a request.
    pub fn route(
        &self,
        method: &http::Method,
        path: &str,
        headers: &http::HeaderMap,
    ) -> Option<RouteMatch> {
        for route in &self.routes {
            if route.matches(method, path, headers) {
                debug!(
                    route = %route.name,
                    upstream = %route.upstream,
                    "matched route"
                );

                let rewritten_path = route.rewrite.as_ref().map(|rewrite| {
                    // Simple rewrite: replace the matched prefix
                    if let PathMatch::Prefix { prefix } = &route.path {
                        path.replacen(prefix, rewrite, 1)
                    } else {
                        rewrite.clone()
                    }
                });

                return Some(RouteMatch {
                    route: route.clone(),
                    rewritten_path,
                });
            }
        }

        // Return default upstream if set
        if let Some(upstream) = &self.default_upstream {
            debug!(upstream = %upstream, "using default upstream");
            return Some(RouteMatch {
                route: Route::new("default", PathMatch::prefix("/"), upstream.clone()),
                rewritten_path: None,
            });
        }

        debug!(path = %path, "no matching route found");
        None
    }

    /// Returns all routes.
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// Returns the number of routes.
    pub fn len(&self) -> usize {
        self.routes.len()
    }

    /// Returns true if there are no routes.
    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

/// Upstream cluster definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamCluster {
    /// Cluster name.
    pub name: String,
    /// List of upstream endpoints.
    pub endpoints: Vec<String>,
    /// Load balancing policy.
    #[serde(default)]
    pub load_balancing: LoadBalancingPolicy,
    /// Health check configuration.
    pub health_check: Option<HealthCheckConfig>,
}

/// Load balancing policy.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancingPolicy {
    /// Round-robin selection.
    #[default]
    RoundRobin,
    /// Least connections.
    LeastConnections,
    /// Random selection.
    Random,
    /// Consistent hashing.
    ConsistentHash,
}

/// Health check configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Health check interval.
    pub interval_ms: u64,
    /// Health check timeout.
    pub timeout_ms: u64,
    /// Path to check.
    pub path: String,
    /// Number of failures before marking unhealthy.
    pub unhealthy_threshold: u32,
    /// Number of successes before marking healthy.
    pub healthy_threshold: u32,
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval_ms: 10000,
            timeout_ms: 5000,
            path: "/health".to_string(),
            unhealthy_threshold: 3,
            healthy_threshold: 2,
        }
    }
}

/// Routing configuration that can be loaded from file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    /// List of routes.
    pub routes: Vec<Route>,
    /// Upstream clusters.
    pub upstreams: HashMap<String, UpstreamCluster>,
    /// Default upstream cluster name.
    pub default_upstream: Option<String>,
}

impl RoutingConfig {
    /// Loads configuration from a TOML string.
    pub fn from_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Loads configuration from a JSON string.
    pub fn from_json(content: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(content)
    }

    /// Builds a router from this configuration.
    pub fn build_router(&self) -> Router {
        let mut router = Router::with_routes(self.routes.clone());
        if let Some(default) = &self.default_upstream {
            router = router.with_default_upstream(default.clone());
        }
        router
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{HeaderMap, HeaderValue, Method};

    #[test]
    fn test_path_match_exact() {
        let matcher = PathMatch::exact("/api/users");
        assert!(matcher.matches("/api/users"));
        assert!(!matcher.matches("/api/users/"));
        assert!(!matcher.matches("/api"));
    }

    #[test]
    fn test_path_match_prefix() {
        let matcher = PathMatch::prefix("/api/");
        assert!(matcher.matches("/api/users"));
        assert!(matcher.matches("/api/posts"));
        assert!(!matcher.matches("/other"));
    }

    #[test]
    fn test_path_match_regex() {
        let matcher = PathMatch::regex(r"^/api/users/\d+$");
        assert!(matcher.matches("/api/users/123"));
        assert!(matcher.matches("/api/users/456"));
        assert!(!matcher.matches("/api/users/abc"));
    }

    #[test]
    fn test_header_match_exact() {
        let matcher = HeaderMatch::exact("content-type", "application/json");
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        assert!(matcher.matches(&headers));

        headers.insert("content-type", HeaderValue::from_static("text/plain"));
        assert!(!matcher.matches(&headers));
    }

    #[test]
    fn test_header_match_present() {
        let matcher = HeaderMatch::present("authorization");
        let mut headers = HeaderMap::new();
        assert!(!matcher.matches(&headers));

        headers.insert("authorization", HeaderValue::from_static("Bearer token"));
        assert!(matcher.matches(&headers));
    }

    #[test]
    fn test_route_matching() {
        let route = Route::new("api-route", PathMatch::prefix("/api/"), "api-cluster")
            .with_method(MethodMatch::Get)
            .with_header(HeaderMatch::present("authorization"));

        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer token"));

        assert!(route.matches(&Method::GET, "/api/users", &headers));
        assert!(!route.matches(&Method::POST, "/api/users", &headers));

        let empty_headers = HeaderMap::new();
        assert!(!route.matches(&Method::GET, "/api/users", &empty_headers));
    }

    #[test]
    fn test_router_priority() {
        let mut router = Router::new();

        // Add routes in non-priority order
        router.add_route(Route::new(
            "prefix",
            PathMatch::prefix("/api/"),
            "prefix-cluster",
        ));
        router.add_route(Route::new(
            "exact",
            PathMatch::exact("/api/users"),
            "exact-cluster",
        ));

        let headers = HeaderMap::new();
        let result = router.route(&Method::GET, "/api/users", &headers);

        // Exact match should be selected
        assert!(result.is_some());
        assert_eq!(result.unwrap().route.name, "exact");
    }

    #[test]
    fn test_router_default_upstream() {
        let router = Router::new().with_default_upstream("default-cluster");

        let headers = HeaderMap::new();
        let result = router.route(&Method::GET, "/unmatched", &headers);

        assert!(result.is_some());
        assert_eq!(result.unwrap().route.upstream, "default-cluster");
    }

    #[test]
    fn test_router_no_match() {
        let router = Router::new();

        let headers = HeaderMap::new();
        let result = router.route(&Method::GET, "/unmatched", &headers);

        assert!(result.is_none());
    }

    #[test]
    fn test_route_rewrite() {
        let route =
            Route::new("rewrite", PathMatch::prefix("/old/"), "cluster").with_rewrite("/new/");

        let mut router = Router::new();
        router.add_route(route);

        let headers = HeaderMap::new();
        let result = router.route(&Method::GET, "/old/path/to/resource", &headers);

        assert!(result.is_some());
        let route_match = result.unwrap();
        assert_eq!(
            route_match.rewritten_path,
            Some("/new/path/to/resource".to_string())
        );
    }
}
