// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Rate-limited, retry-aware, circuit-broken HTTP transport client.
//!
//! [`TransportClient`] wraps the same `ureq`-based HTTP machinery as
//! [`super::http_get_text`] but adds per-domain operational intelligence:
//!
//! - **Rate limiting** — sliding-window + minimum-interval enforcement
//! - **Circuit breaking** — automatic back-off from persistently failing endpoints
//! - **Retry with backoff** — exponential/linear/fixed with jitter, honours `Retry-After`
//! - **Response metadata** — timing, status, retry count, rate-limit headers
//! - **Source-class presets** — tiered policies matching OSINT risk categories
//!
//! # Example
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use skeletrace::transport::client::{TransportClient, SourceClass};
//! use skeletrace::transport::HttpRequestProfile;
//!
//! let mut client = TransportClient::new();
//! client.register_domain("api.example.com", SourceClass::SafeDefault);
//! client.register_domain("secret.onion", SourceClass::Gated);
//!
//! let profile = HttpRequestProfile::direct(Duration::from_secs(10));
//! match client.get_text("https://api.example.com/v1/data", &profile) {
//!     Ok(resp) => println!("got {} bytes in {:?}", resp.body.len(), resp.meta.total_duration),
//!     Err(e) => eprintln!("transport error: {e}"),
//! }
//! ```

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::circuit_breaker::{CircuitBreaker, CircuitBreakerPolicy, CircuitCheck, CircuitState};
use super::rate_limit::{RateLimitOutcome, RateLimitPolicy, RateLimiter};
use super::retry::{evaluate_retry, parse_retry_after, RetryDecision, RetryPolicy};
use super::{apply_auth, extract_domain, HttpRequestProfile, TransportError};

type HttpResponseParts = (u16, Vec<(String, String)>, String);
type RequestFailure = (bool, Option<u16>, String, Option<u64>);

// ─── Source class presets ────────────────────────────────────────────────────

/// Pre-bundled policy tier for a source, matching the OSINT risk categories.
///
/// See the Perplexity source-landscape analysis for context on the three
/// standard tiers plus the feed-specific tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SourceClass {
    /// Registries, sanctions lists, RDAP, well-provisioned public APIs.
    /// Aggressive polling allowed.
    SafeDefault,
    /// RSS/Atom feed polling. Moderate rate, moderate tolerance.
    Feed,
    /// Social scraping, DNS/passive, rate-limited APIs. Cautious rates.
    Controlled,
    /// Breach corpora, onion services, leak sources. Minimal, stealthy.
    Gated,
    /// Fully custom policies.
    Custom {
        rate_limit: RateLimitPolicy,
        circuit_breaker: CircuitBreakerPolicy,
        retry: RetryPolicy,
    },
}

impl SourceClass {
    /// Decompose into the three policy components.
    #[must_use]
    pub fn policies(&self) -> (RateLimitPolicy, CircuitBreakerPolicy, RetryPolicy) {
        match self {
            Self::SafeDefault => (
                RateLimitPolicy::generous(),
                CircuitBreakerPolicy::tolerant(),
                RetryPolicy::standard(),
            ),
            Self::Feed => (
                RateLimitPolicy::feed(),
                CircuitBreakerPolicy::balanced(),
                RetryPolicy::cautious(),
            ),
            Self::Controlled => (
                RateLimitPolicy::cautious(),
                CircuitBreakerPolicy::balanced(),
                RetryPolicy::cautious(),
            ),
            Self::Gated => (
                RateLimitPolicy::stealth(),
                CircuitBreakerPolicy::sensitive(),
                RetryPolicy::minimal(),
            ),
            Self::Custom {
                rate_limit,
                circuit_breaker,
                retry,
            } => (rate_limit.clone(), circuit_breaker.clone(), retry.clone()),
        }
    }
}

// ─── Response metadata ───────────────────────────────────────────────────────

/// Metadata about a completed (or failed) HTTP request cycle.
#[derive(Debug, Clone)]
pub struct ResponseMeta {
    /// HTTP status code of the final response.
    pub status: u16,
    /// Wall-clock time for the entire request cycle (including retries).
    pub total_duration: Duration,
    /// Wall-clock time for just the final successful request.
    pub request_duration: Duration,
    /// Number of retry attempts before success (0 = first try worked).
    pub retry_count: u32,
    /// Parsed `X-RateLimit-Remaining` if the server sent one.
    pub rate_limit_remaining: Option<u32>,
    /// The domain key used for rate-limit / circuit-breaker tracking.
    pub domain: String,
    /// Which proxy route was used.
    pub proxy_route_used: String,
    /// Circuit state at the time of the request.
    pub circuit_state: CircuitState,
}

/// Successful transport response.
#[derive(Debug, Clone)]
pub struct TransportResponse {
    /// Response body as UTF-8 text.
    pub body: String,
    /// Request metadata.
    pub meta: ResponseMeta,
}

// ─── Client ──────────────────────────────────────────────────────────────────

/// Production HTTP client with rate limiting, circuit breaking, and retry.
///
/// Designed to replace [`super::http_get_text`] for source polling loops.
/// All operational state (rate windows, circuit states) is held in-memory
/// and resets when the client is dropped.
pub struct TransportClient {
    rate_limiter: RateLimiter,
    circuit_breaker: CircuitBreaker,
    retry_policies: HashMap<String, RetryPolicy>,
    default_retry: RetryPolicy,
    /// Maximum time to sleep waiting for a rate-limit window before
    /// giving up with a `RateLimited` error.
    max_rate_wait: Duration,
    /// Whether to actually sleep (true) or return `RateLimited` error
    /// immediately (false). Useful for testing.
    block_on_rate_limit: bool,
}

impl TransportClient {
    /// Create a client with sensible defaults for all policies.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate_limiter: RateLimiter::default(),
            circuit_breaker: CircuitBreaker::default(),
            retry_policies: HashMap::new(),
            default_retry: RetryPolicy::default(),
            max_rate_wait: Duration::from_secs(120),
            block_on_rate_limit: true,
        }
    }

    // ── Builder-style configuration ──────────────────────────────────────

    /// Override the default rate-limit policy for unregistered domains.
    #[must_use]
    pub fn with_default_rate_policy(mut self, policy: RateLimitPolicy) -> Self {
        self.rate_limiter = RateLimiter::new(policy);
        self
    }

    /// Override the default circuit-breaker policy.
    #[must_use]
    pub fn with_default_circuit_policy(mut self, policy: CircuitBreakerPolicy) -> Self {
        self.circuit_breaker = CircuitBreaker::new(policy);
        self
    }

    /// Override the default retry policy.
    #[must_use]
    pub fn with_default_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.default_retry = policy;
        self
    }

    /// Maximum time the client will block waiting for a rate-limit window.
    /// If the wait would exceed this, a `RateLimited` error is returned
    /// immediately.
    #[must_use]
    pub fn with_max_rate_wait(mut self, max: Duration) -> Self {
        self.max_rate_wait = max;
        self
    }

    /// If `false`, rate-limit violations return `RateLimited` immediately
    /// instead of sleeping. Useful for non-blocking engine designs.
    #[must_use]
    pub fn with_block_on_rate_limit(mut self, block: bool) -> Self {
        self.block_on_rate_limit = block;
        self
    }

    // ── Domain registration ──────────────────────────────────────────────

    /// Register a domain with a bundled source-class preset.
    pub fn register_domain(&mut self, domain: &str, class: SourceClass) {
        let (rl, cb, retry) = class.policies();
        self.rate_limiter.set_domain_policy(domain, rl);
        self.circuit_breaker.set_domain_policy(domain, cb);
        self.retry_policies.insert(domain.to_owned(), retry);
    }

    /// Register a domain with individual policy components.
    pub fn register_domain_custom(
        &mut self,
        domain: &str,
        rate_limit: RateLimitPolicy,
        circuit_breaker: CircuitBreakerPolicy,
        retry: RetryPolicy,
    ) {
        self.rate_limiter.set_domain_policy(domain, rate_limit);
        self.circuit_breaker
            .set_domain_policy(domain, circuit_breaker);
        self.retry_policies.insert(domain.to_owned(), retry);
    }

    // ── Observability ────────────────────────────────────────────────────

    /// Current circuit state for a domain.
    #[must_use]
    pub fn circuit_state(&self, domain: &str) -> CircuitState {
        self.circuit_breaker.state(domain)
    }

    /// Current number of tracked requests in a domain's rate-limit window.
    #[must_use]
    pub fn rate_limit_count(&self, domain: &str) -> u32 {
        self.rate_limiter.current_count(domain)
    }

    /// Total failures recorded for a domain.
    #[must_use]
    pub fn total_failures(&self, domain: &str) -> u64 {
        self.circuit_breaker.total_failures(domain)
    }

    /// Total successes recorded for a domain.
    #[must_use]
    pub fn total_successes(&self, domain: &str) -> u64 {
        self.circuit_breaker.total_successes(domain)
    }

    /// Reset all operational state (rate windows, circuit breakers).
    pub fn reset_all(&mut self) {
        self.rate_limiter.reset_all();
        self.circuit_breaker.reset_all();
    }

    /// Reset operational state for a single domain.
    pub fn reset_domain(&mut self, domain: &str) {
        self.rate_limiter.reset_domain(domain);
        self.circuit_breaker.reset_domain(domain);
    }

    // ── Core request method ──────────────────────────────────────────────

    /// Perform an HTTP GET with full operational intelligence.
    ///
    /// Applies rate limiting, circuit breaking, and retry logic according
    /// to the domain's registered policies. Returns the response body and
    /// metadata on success.
    pub fn get_text(
        &mut self,
        url: &str,
        profile: &HttpRequestProfile,
    ) -> Result<TransportResponse, TransportError> {
        profile.validate()?;

        let domain = extract_domain(url);
        let cycle_start = Instant::now();

        // 1. Circuit breaker check.
        let circuit_state = match self.circuit_breaker.check(&domain) {
            CircuitCheck::Allowed(state) => state,
            CircuitCheck::Blocked { remaining_cooldown } => {
                return Err(TransportError::CircuitOpen {
                    domain,
                    remaining_cooldown_ms: remaining_cooldown.as_millis() as u64,
                });
            }
        };

        // 2. Rate-limit check (may sleep).
        self.wait_for_rate_limit(&domain)?;

        // 3. Execute with retry loop.
        let retry_policy = self
            .retry_policies
            .get(&domain)
            .cloned()
            .unwrap_or_else(|| self.default_retry.clone());

        let mut last_error = String::new();

        for attempt in 0..=retry_policy.max_retries {
            // Record the request in the rate limiter.
            self.rate_limiter.record(&domain);

            let req_start = Instant::now();
            match self.execute_request(url, profile) {
                Ok((status, headers, body)) => {
                    let req_duration = req_start.elapsed();
                    self.circuit_breaker.record_success(&domain);

                    let rate_limit_remaining = headers
                        .iter()
                        .find(|(k, _)| {
                            let lower = k.to_lowercase();
                            lower == "x-ratelimit-remaining"
                                || lower == "x-rate-limit-remaining"
                                || lower == "ratelimit-remaining"
                        })
                        .and_then(|(_, v)| v.parse::<u32>().ok());

                    return Ok(TransportResponse {
                        body,
                        meta: ResponseMeta {
                            status,
                            total_duration: cycle_start.elapsed(),
                            request_duration: req_duration,
                            retry_count: attempt,
                            rate_limit_remaining,
                            domain,
                            proxy_route_used: profile.proxy_route.label().to_owned(),
                            circuit_state,
                        },
                    });
                }
                Err((retriable, status_code, error_msg, retry_after)) => {
                    last_error = error_msg;
                    self.circuit_breaker.record_failure(&domain);

                    let is_retriable = retriable
                        && (retry_policy.retry_on_transport_error
                            || status_code.is_some_and(|s| retry_policy.should_retry_status(s)));

                    match evaluate_retry(&retry_policy, attempt, is_retriable, retry_after) {
                        RetryDecision::Retry(delay) => {
                            // Wait for rate limit again before retry.
                            std::thread::sleep(delay);
                            self.wait_for_rate_limit(&domain)?;
                        }
                        RetryDecision::GiveUp => break,
                    }
                }
            }
        }

        Err(TransportError::RetriesExhausted {
            domain,
            attempts: retry_policy.max_retries + 1,
            last_error,
        })
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    /// Block until the rate limiter allows a request, or error if the
    /// wait would exceed `max_rate_wait`.
    fn wait_for_rate_limit(&mut self, domain: &str) -> Result<(), TransportError> {
        loop {
            match self.rate_limiter.check(domain) {
                RateLimitOutcome::Allowed => return Ok(()),
                RateLimitOutcome::Wait(wait) => {
                    if wait > self.max_rate_wait {
                        return Err(TransportError::RateLimited {
                            domain: domain.to_owned(),
                            wait_ms: wait.as_millis() as u64,
                        });
                    }
                    if !self.block_on_rate_limit {
                        return Err(TransportError::RateLimited {
                            domain: domain.to_owned(),
                            wait_ms: wait.as_millis() as u64,
                        });
                    }
                    std::thread::sleep(wait);
                }
            }
        }
    }

    /// Execute a single HTTP GET and return parsed results.
    ///
    /// Returns:
    /// - `Ok((status, headers, body))` on any 2xx response
    /// - `Err((retriable, status_code, error_msg, retry_after_secs))` on failure
    fn execute_request(
        &self,
        url: &str,
        profile: &HttpRequestProfile,
    ) -> Result<HttpResponseParts, RequestFailure> {
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(profile.timeout)
            .timeout_read(profile.timeout)
            .timeout_write(profile.timeout);

        if let Some(proxy_url) = profile.proxy_route.proxy_url() {
            let proxy = ureq::Proxy::new(proxy_url)
                .map_err(|err| (false, None, format!("proxy config error: {err}"), None))?;
            builder = builder.proxy(proxy);
        }

        let agent = builder.build();
        let mut request = agent.get(url);

        if let Some(user_agent) = &profile.user_agent {
            request = request.set("User-Agent", user_agent);
        }
        for header in &profile.headers {
            request = request.set(&header.name, &header.value);
        }
        if let Some(auth) = &profile.auth {
            request = apply_auth(request, auth);
        }

        match request.call() {
            Ok(response) => {
                let status = response.status();
                let headers = extract_response_headers(&response);
                let body = response
                    .into_string()
                    .map_err(|err| (false, Some(status), err.to_string(), None))?;
                Ok((status, headers, body))
            }
            Err(ureq::Error::Status(code, response)) => {
                let retry_after = response.header("Retry-After").and_then(parse_retry_after);
                let msg = response
                    .into_string()
                    .unwrap_or_else(|_| format!("HTTP {code}"));
                // 4xx other than 429 are not retriable; 5xx and 429 are.
                let retriable = code == 429 || (500..600).contains(&code);
                Err((retriable, Some(code), msg, retry_after))
            }
            Err(ureq::Error::Transport(transport)) => {
                let msg = transport.to_string();
                Err((true, None, msg, None))
            }
        }
    }
}

impl Default for TransportClient {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for TransportClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportClient")
            .field("max_rate_wait", &self.max_rate_wait)
            .field("block_on_rate_limit", &self.block_on_rate_limit)
            .finish_non_exhaustive()
    }
}

// ─── Header extraction helper ────────────────────────────────────────────────

/// Pull response headers into owned key-value pairs.
///
/// `ureq::Response` exposes headers via `header()` / `headers_names()`.
fn extract_response_headers(response: &ureq::Response) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            out.push((name, value.to_owned()));
        }
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::ProxyRoute;

    #[test]
    fn client_creation_defaults() {
        let client = TransportClient::new();
        assert!(client.block_on_rate_limit);
        assert_eq!(client.max_rate_wait, Duration::from_secs(120));
    }

    #[test]
    fn builder_configuration() {
        let client = TransportClient::new()
            .with_max_rate_wait(Duration::from_secs(30))
            .with_block_on_rate_limit(false)
            .with_default_retry_policy(RetryPolicy::none());

        assert!(!client.block_on_rate_limit);
        assert_eq!(client.max_rate_wait, Duration::from_secs(30));
    }

    #[test]
    fn domain_registration() {
        let mut client = TransportClient::new();
        client.register_domain("api.example.com", SourceClass::SafeDefault);
        client.register_domain("dark.onion", SourceClass::Gated);

        assert_eq!(
            client.circuit_state("api.example.com"),
            CircuitState::Closed
        );
        assert_eq!(client.circuit_state("dark.onion"), CircuitState::Closed);
    }

    #[test]
    fn source_class_policies_decompose() {
        let (rl, cb, retry) = SourceClass::SafeDefault.policies();
        assert_eq!(rl.max_requests, 60);
        assert_eq!(cb.failure_threshold, 10);
        assert_eq!(retry.max_retries, 3);

        let (rl, cb, retry) = SourceClass::Gated.policies();
        assert_eq!(rl.max_requests, 2);
        assert_eq!(cb.failure_threshold, 3);
        assert_eq!(retry.max_retries, 1);

        let (rl, cb, retry) = SourceClass::Feed.policies();
        assert_eq!(rl.max_requests, 20);
        assert_eq!(cb.failure_threshold, 5);
        assert_eq!(retry.max_retries, 2);

        let (rl, cb, retry) = SourceClass::Controlled.policies();
        assert_eq!(rl.max_requests, 10);
        assert_eq!(cb.failure_threshold, 5);
        assert_eq!(retry.max_retries, 2);
    }

    #[test]
    fn rejects_invalid_profile() {
        let mut client = TransportClient::new();
        let bad_profile = HttpRequestProfile {
            timeout: Duration::ZERO,
            headers: Vec::new(),
            auth: None,
            proxy_route: ProxyRoute::Direct,
            user_agent: None,
        };
        let result = client.get_text("https://example.com", &bad_profile);
        assert!(matches!(result, Err(TransportError::Validation(_))));
    }

    #[test]
    fn non_blocking_rate_limit_returns_error() {
        let mut client = TransportClient::new()
            .with_block_on_rate_limit(false)
            .with_default_rate_policy(RateLimitPolicy::new(
                1,
                Duration::from_secs(60),
                Duration::from_secs(60),
            ));

        let profile = HttpRequestProfile::direct(Duration::from_secs(5));

        // First request uses rate limiter — will be allowed and then fail
        // on network (that's fine, we're testing rate limiting).
        let _ = client.get_text("https://localhost:1/fake", &profile);

        // Second request should be rate-limited immediately.
        let result = client.get_text("https://localhost:1/fake", &profile);
        assert!(
            matches!(result, Err(TransportError::RateLimited { .. })),
            "expected RateLimited, got {:?}",
            result,
        );
    }

    #[test]
    fn observability_counters() {
        let client = TransportClient::new();
        assert_eq!(client.total_failures("example.com"), 0);
        assert_eq!(client.total_successes("example.com"), 0);
        assert_eq!(client.rate_limit_count("example.com"), 0);
    }

    #[test]
    fn reset_domain_clears_state() {
        let mut client = TransportClient::new();
        client.register_domain("a.com", SourceClass::SafeDefault);
        // Simulate some tracking.
        client.rate_limiter.record("a.com");
        assert_eq!(client.rate_limit_count("a.com"), 1);

        client.reset_domain("a.com");
        assert_eq!(client.rate_limit_count("a.com"), 0);
    }
}
