// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Per-domain sliding-window rate limiter.
//!
//! Each domain gets an independent window that tracks recent request
//! timestamps. When a request would exceed the configured limit, the
//! limiter returns how long the caller must wait before proceeding.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ─── Policy ──────────────────────────────────────────────────────────────────

/// Controls how aggressively a domain may be polled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitPolicy {
    /// Maximum requests allowed within [`window`].
    pub max_requests: u32,
    /// Sliding window duration over which `max_requests` is counted.
    pub window: Duration,
    /// Minimum delay between consecutive requests to the same domain.
    /// Applied independently of the sliding window.
    pub min_interval: Duration,
}

impl RateLimitPolicy {
    #[must_use]
    pub fn new(max_requests: u32, window: Duration, min_interval: Duration) -> Self {
        Self {
            max_requests,
            window,
            min_interval,
        }
    }

    /// 60 req/min, no min interval. Well-provisioned public APIs.
    #[must_use]
    pub fn generous() -> Self {
        Self::new(60, Duration::from_secs(60), Duration::ZERO)
    }

    /// 10 req/min, 2 s between requests. Rate-limited APIs, scraping.
    #[must_use]
    pub fn cautious() -> Self {
        Self::new(10, Duration::from_secs(60), Duration::from_secs(2))
    }

    /// 2 req/min, 15 s between requests. Onion services, stealth.
    #[must_use]
    pub fn stealth() -> Self {
        Self::new(2, Duration::from_secs(60), Duration::from_secs(15))
    }

    /// 20 req/min, 3 s between requests. RSS/Atom feed polling.
    #[must_use]
    pub fn feed() -> Self {
        Self::new(20, Duration::from_secs(60), Duration::from_secs(3))
    }
}

impl Default for RateLimitPolicy {
    /// 30 req/min, 1 s between requests.
    fn default() -> Self {
        Self::new(30, Duration::from_secs(60), Duration::from_secs(1))
    }
}

// ─── Outcome ─────────────────────────────────────────────────────────────────

/// Result of a rate-limit check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitOutcome {
    /// The request may proceed immediately.
    Allowed,
    /// The caller must wait at least this long before retrying.
    Wait(Duration),
}

// ─── Per-domain state ────────────────────────────────────────────────────────

#[derive(Debug)]
struct DomainWindow {
    /// Timestamps of requests within the current window.
    timestamps: Vec<Instant>,
    /// When the most recent request was issued.
    last_request: Option<Instant>,
}

impl DomainWindow {
    fn new() -> Self {
        Self {
            timestamps: Vec::new(),
            last_request: None,
        }
    }

    /// Remove timestamps that have fallen outside the window.
    fn prune(&mut self, now: Instant, window: Duration) {
        self.timestamps
            .retain(|ts| now.duration_since(*ts) <= window);
    }

    /// Number of requests currently inside the window.
    fn count(&self) -> u32 {
        self.timestamps.len() as u32
    }

    /// Record that a request was issued right now.
    fn record(&mut self, now: Instant) {
        self.timestamps.push(now);
        self.last_request = Some(now);
    }

    /// Time until the oldest request in the window expires, making room
    /// for a new one.
    fn time_until_slot(&self, now: Instant, window: Duration) -> Duration {
        if let Some(oldest) = self.timestamps.first() {
            let age = now.duration_since(*oldest);
            if age < window {
                return window - age;
            }
        }
        Duration::ZERO
    }

    /// Time remaining on the minimum inter-request interval.
    fn time_until_interval(&self, now: Instant, min_interval: Duration) -> Duration {
        if let Some(last) = self.last_request {
            let elapsed = now.duration_since(last);
            if elapsed < min_interval {
                return min_interval - elapsed;
            }
        }
        Duration::ZERO
    }
}

// ─── Limiter ─────────────────────────────────────────────────────────────────

/// Per-domain sliding-window rate limiter.
///
/// Maintains independent request windows for each domain. Domains that
/// have no explicit policy fall back to the configured default.
#[derive(Debug)]
pub struct RateLimiter {
    domains: HashMap<String, DomainWindow>,
    default_policy: RateLimitPolicy,
    domain_policies: HashMap<String, RateLimitPolicy>,
}

impl RateLimiter {
    /// Create a limiter with the given default policy.
    #[must_use]
    pub fn new(default_policy: RateLimitPolicy) -> Self {
        Self {
            domains: HashMap::new(),
            default_policy,
            domain_policies: HashMap::new(),
        }
    }

    /// Register a per-domain policy override.
    pub fn set_domain_policy(&mut self, domain: impl Into<String>, policy: RateLimitPolicy) {
        self.domain_policies.insert(domain.into(), policy);
    }

    /// Look up which policy applies to a domain.
    #[must_use]
    pub fn policy_for(&self, domain: &str) -> &RateLimitPolicy {
        self.domain_policies
            .get(domain)
            .unwrap_or(&self.default_policy)
    }

    /// Check whether a request to `domain` is allowed right now.
    ///
    /// Does **not** record the request — call [`record`] after the
    /// request actually fires.
    pub fn check(&mut self, domain: &str) -> RateLimitOutcome {
        let now = Instant::now();
        let policy = self
            .domain_policies
            .get(domain)
            .cloned()
            .unwrap_or_else(|| self.default_policy.clone());

        let window = self
            .domains
            .entry(domain.to_owned())
            .or_insert_with(DomainWindow::new);

        window.prune(now, policy.window);

        // Check minimum interval first.
        let interval_wait = window.time_until_interval(now, policy.min_interval);
        if !interval_wait.is_zero() {
            return RateLimitOutcome::Wait(interval_wait);
        }

        // Check sliding window capacity.
        if window.count() >= policy.max_requests {
            let slot_wait = window.time_until_slot(now, policy.window);
            return RateLimitOutcome::Wait(if slot_wait.is_zero() {
                // Shouldn't happen after prune, but be safe.
                Duration::from_millis(100)
            } else {
                slot_wait
            });
        }

        RateLimitOutcome::Allowed
    }

    /// Record that a request to `domain` was just issued.
    pub fn record(&mut self, domain: &str) {
        let now = Instant::now();
        self.domains
            .entry(domain.to_owned())
            .or_insert_with(DomainWindow::new)
            .record(now);
    }

    /// How many requests are currently tracked for a domain.
    #[must_use]
    pub fn current_count(&self, domain: &str) -> u32 {
        self.domains.get(domain).map_or(0, |w| w.count())
    }

    /// Forget all state for a domain (useful after config changes).
    pub fn reset_domain(&mut self, domain: &str) {
        self.domains.remove(domain);
    }

    /// Forget all tracking state.
    pub fn reset_all(&mut self) {
        self.domains.clear();
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RateLimitPolicy::default())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_first_request() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::generous());
        assert_eq!(limiter.check("example.com"), RateLimitOutcome::Allowed);
    }

    #[test]
    fn enforces_min_interval() {
        let policy = RateLimitPolicy::new(100, Duration::from_secs(60), Duration::from_millis(500));
        let mut limiter = RateLimiter::new(policy);

        assert_eq!(limiter.check("a.com"), RateLimitOutcome::Allowed);
        limiter.record("a.com");

        // Immediately after, should be told to wait.
        match limiter.check("a.com") {
            RateLimitOutcome::Wait(d) => assert!(d.as_millis() > 0),
            RateLimitOutcome::Allowed => panic!("should have been rate-limited"),
        }
    }

    #[test]
    fn enforces_window_capacity() {
        let policy = RateLimitPolicy::new(2, Duration::from_secs(60), Duration::ZERO);
        let mut limiter = RateLimiter::new(policy);

        assert_eq!(limiter.check("b.com"), RateLimitOutcome::Allowed);
        limiter.record("b.com");
        assert_eq!(limiter.check("b.com"), RateLimitOutcome::Allowed);
        limiter.record("b.com");

        // Third request should be denied.
        match limiter.check("b.com") {
            RateLimitOutcome::Wait(_) => {}
            RateLimitOutcome::Allowed => panic!("should have been rate-limited"),
        }
    }

    #[test]
    fn independent_domains() {
        let policy = RateLimitPolicy::new(1, Duration::from_secs(60), Duration::ZERO);
        let mut limiter = RateLimiter::new(policy);

        assert_eq!(limiter.check("x.com"), RateLimitOutcome::Allowed);
        limiter.record("x.com");

        // Different domain is independent.
        assert_eq!(limiter.check("y.com"), RateLimitOutcome::Allowed);
    }

    #[test]
    fn domain_policy_override() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::stealth());
        limiter.set_domain_policy("fast.com", RateLimitPolicy::generous());

        // fast.com should use the generous policy.
        assert_eq!(limiter.check("fast.com"), RateLimitOutcome::Allowed);
        limiter.record("fast.com");
        // With generous policy (no min_interval), immediate re-request is fine.
        assert_eq!(limiter.check("fast.com"), RateLimitOutcome::Allowed);
    }

    #[test]
    fn reset_domain_clears_state() {
        let policy = RateLimitPolicy::new(1, Duration::from_secs(60), Duration::ZERO);
        let mut limiter = RateLimiter::new(policy);

        limiter.record("z.com");
        assert!(matches!(limiter.check("z.com"), RateLimitOutcome::Wait(_)));

        limiter.reset_domain("z.com");
        assert_eq!(limiter.check("z.com"), RateLimitOutcome::Allowed);
    }
}
