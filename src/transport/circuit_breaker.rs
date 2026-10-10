// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Per-domain circuit breaker.
//!
//! Prevents hammering endpoints that are consistently failing.
//! Each domain transitions through three states:
//!
//! - **Closed** — healthy, requests flow normally.
//! - **Open** — failure threshold reached, requests are blocked for a
//!   cooldown period.
//! - **HalfOpen** — cooldown elapsed, one probe request is allowed to
//!   test whether the endpoint has recovered.
//!
//! On probe success the circuit closes; on probe failure it re-opens.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

// ─── Policy ──────────────────────────────────────────────────────────────────

/// Controls when a circuit trips and how long it stays open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerPolicy {
    /// Number of consecutive failures before the circuit opens.
    pub failure_threshold: u32,
    /// How long the circuit stays open before allowing a probe.
    pub cooldown: Duration,
}

impl CircuitBreakerPolicy {
    #[must_use]
    pub fn new(failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            failure_threshold,
            cooldown,
        }
    }

    /// 10 failures, 60 s cooldown. Tolerant — for stable public APIs.
    #[must_use]
    pub fn tolerant() -> Self {
        Self::new(10, Duration::from_secs(60))
    }

    /// 5 failures, 120 s cooldown. Balanced — for rate-limited / scraped sources.
    #[must_use]
    pub fn balanced() -> Self {
        Self::new(5, Duration::from_secs(120))
    }

    /// 3 failures, 300 s cooldown. Sensitive — for onion / high-risk sources.
    #[must_use]
    pub fn sensitive() -> Self {
        Self::new(3, Duration::from_secs(300))
    }
}

impl Default for CircuitBreakerPolicy {
    fn default() -> Self {
        Self::balanced()
    }
}

// ─── State ───────────────────────────────────────────────────────────────────

/// Observable circuit state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// Check result returned to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitCheck {
    /// Request may proceed.
    Allowed(CircuitState),
    /// Circuit is open; caller must wait.
    Blocked { remaining_cooldown: Duration },
}

// ─── Per-domain tracker ──────────────────────────────────────────────────────

#[derive(Debug)]
struct DomainCircuit {
    consecutive_failures: u32,
    state: CircuitState,
    opened_at: Option<Instant>,
    last_failure: Option<Instant>,
    last_success: Option<Instant>,
    total_failures: u64,
    total_successes: u64,
}

impl DomainCircuit {
    fn new() -> Self {
        Self {
            consecutive_failures: 0,
            state: CircuitState::Closed,
            opened_at: None,
            last_failure: None,
            last_success: None,
            total_failures: 0,
            total_successes: 0,
        }
    }

    fn check(&mut self, now: Instant, policy: &CircuitBreakerPolicy) -> CircuitCheck {
        match self.state {
            CircuitState::Closed => CircuitCheck::Allowed(CircuitState::Closed),
            CircuitState::Open => {
                if let Some(opened) = self.opened_at {
                    let elapsed = now.duration_since(opened);
                    if elapsed >= policy.cooldown {
                        self.state = CircuitState::HalfOpen;
                        CircuitCheck::Allowed(CircuitState::HalfOpen)
                    } else {
                        CircuitCheck::Blocked {
                            remaining_cooldown: policy.cooldown - elapsed,
                        }
                    }
                } else {
                    // Shouldn't happen, but treat as closed.
                    self.state = CircuitState::Closed;
                    CircuitCheck::Allowed(CircuitState::Closed)
                }
            }
            CircuitState::HalfOpen => {
                // Only one probe at a time — if we're already half-open
                // and called again, it means the probe hasn't resolved yet.
                // Allow it (the engine is single-threaded today).
                CircuitCheck::Allowed(CircuitState::HalfOpen)
            }
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.state = CircuitState::Closed;
        self.opened_at = None;
        self.last_success = Some(Instant::now());
        self.total_successes += 1;
    }

    fn record_failure(&mut self, now: Instant, policy: &CircuitBreakerPolicy) {
        self.consecutive_failures += 1;
        self.last_failure = Some(now);
        self.total_failures += 1;

        if self.consecutive_failures >= policy.failure_threshold {
            self.state = CircuitState::Open;
            self.opened_at = Some(now);
        }
    }
}

// ─── Breaker ─────────────────────────────────────────────────────────────────

/// Per-domain circuit breaker.
///
/// Domains without an explicit policy fall back to the configured default.
#[derive(Debug)]
pub struct CircuitBreaker {
    domains: HashMap<String, DomainCircuit>,
    default_policy: CircuitBreakerPolicy,
    domain_policies: HashMap<String, CircuitBreakerPolicy>,
}

impl CircuitBreaker {
    #[must_use]
    pub fn new(default_policy: CircuitBreakerPolicy) -> Self {
        Self {
            domains: HashMap::new(),
            default_policy,
            domain_policies: HashMap::new(),
        }
    }

    /// Register a per-domain policy override.
    pub fn set_domain_policy(&mut self, domain: impl Into<String>, policy: CircuitBreakerPolicy) {
        self.domain_policies.insert(domain.into(), policy);
    }

    /// Check whether a request to `domain` should proceed.
    pub fn check(&mut self, domain: &str) -> CircuitCheck {
        let now = Instant::now();
        let policy = self
            .domain_policies
            .get(domain)
            .cloned()
            .unwrap_or_else(|| self.default_policy.clone());

        self.domains
            .entry(domain.to_owned())
            .or_insert_with(DomainCircuit::new)
            .check(now, &policy)
    }

    /// Record a successful request.
    pub fn record_success(&mut self, domain: &str) {
        self.domains
            .entry(domain.to_owned())
            .or_insert_with(DomainCircuit::new)
            .record_success();
    }

    /// Record a failed request.
    pub fn record_failure(&mut self, domain: &str) {
        let now = Instant::now();
        let policy = self
            .domain_policies
            .get(domain)
            .cloned()
            .unwrap_or_else(|| self.default_policy.clone());

        self.domains
            .entry(domain.to_owned())
            .or_insert_with(DomainCircuit::new)
            .record_failure(now, &policy);
    }

    /// Current state of a domain's circuit.
    #[must_use]
    pub fn state(&self, domain: &str) -> CircuitState {
        self.domains
            .get(domain)
            .map_or(CircuitState::Closed, |c| c.state)
    }

    /// Total failure count for a domain (lifetime, not just consecutive).
    #[must_use]
    pub fn total_failures(&self, domain: &str) -> u64 {
        self.domains.get(domain).map_or(0, |c| c.total_failures)
    }

    /// Total success count for a domain.
    #[must_use]
    pub fn total_successes(&self, domain: &str) -> u64 {
        self.domains.get(domain).map_or(0, |c| c.total_successes)
    }

    /// Reset a single domain's circuit to closed with zero failures.
    pub fn reset_domain(&mut self, domain: &str) {
        self.domains.remove(domain);
    }

    /// Reset all circuits.
    pub fn reset_all(&mut self) {
        self.domains.clear();
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(CircuitBreakerPolicy::default())
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let mut cb = CircuitBreaker::default();
        assert!(matches!(
            cb.check("a.com"),
            CircuitCheck::Allowed(CircuitState::Closed)
        ));
    }

    #[test]
    fn opens_after_threshold() {
        let policy = CircuitBreakerPolicy::new(3, Duration::from_secs(60));
        let mut cb = CircuitBreaker::new(policy);

        cb.record_failure("a.com");
        cb.record_failure("a.com");
        assert!(matches!(cb.check("a.com"), CircuitCheck::Allowed(_)));

        cb.record_failure("a.com");
        assert!(matches!(cb.check("a.com"), CircuitCheck::Blocked { .. }));
    }

    #[test]
    fn success_resets_failures() {
        let policy = CircuitBreakerPolicy::new(3, Duration::from_secs(60));
        let mut cb = CircuitBreaker::new(policy);

        cb.record_failure("a.com");
        cb.record_failure("a.com");
        cb.record_success("a.com");
        cb.record_failure("a.com");
        cb.record_failure("a.com");

        // Still should be allowed — success reset the consecutive count.
        assert!(matches!(cb.check("a.com"), CircuitCheck::Allowed(_)));
    }

    #[test]
    fn independent_domains() {
        let policy = CircuitBreakerPolicy::new(2, Duration::from_secs(60));
        let mut cb = CircuitBreaker::new(policy);

        cb.record_failure("bad.com");
        cb.record_failure("bad.com");

        assert!(matches!(cb.check("bad.com"), CircuitCheck::Blocked { .. }));
        assert!(matches!(cb.check("good.com"), CircuitCheck::Allowed(_)));
    }

    #[test]
    fn counts_track_correctly() {
        let mut cb = CircuitBreaker::default();

        cb.record_success("a.com");
        cb.record_success("a.com");
        cb.record_failure("a.com");

        assert_eq!(cb.total_successes("a.com"), 2);
        assert_eq!(cb.total_failures("a.com"), 1);
    }

    #[test]
    fn reset_clears_state() {
        let policy = CircuitBreakerPolicy::new(1, Duration::from_secs(60));
        let mut cb = CircuitBreaker::new(policy);

        cb.record_failure("a.com");
        assert!(matches!(cb.check("a.com"), CircuitCheck::Blocked { .. }));

        cb.reset_domain("a.com");
        assert!(matches!(cb.check("a.com"), CircuitCheck::Allowed(_)));
    }
}
