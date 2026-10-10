// Copyright (C) 2026 whispr.dev
// SPDX-License-Identifier: GPL-3.0-only

//! Retry and backoff policies.
//!
//! Configurable strategies for retrying failed HTTP requests with
//! exponential, linear, or fixed backoff and pseudo-random jitter.

use std::time::Duration;

use serde::{Deserialize, Serialize};

// ─── Backoff strategy ────────────────────────────────────────────────────────

/// How delay increases between retry attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    /// Delay doubles each attempt: `base * 2^attempt`, capped at `max`.
    Exponential { base: Duration, max: Duration },
    /// Delay increases linearly: `step * (attempt + 1)`, capped at `max`.
    Linear { step: Duration, max: Duration },
    /// Constant delay between attempts.
    Fixed(Duration),
}

impl BackoffStrategy {
    /// Compute the raw delay for a given zero-indexed attempt.
    #[must_use]
    pub fn delay_for(&self, attempt: u32) -> Duration {
        match self {
            Self::Exponential { base, max } => {
                // For attempt >= 32 the multiplier overflows u32, so
                // cap directly at max to avoid silent truncation to zero.
                if attempt >= 32 {
                    return *max;
                }
                let multiplier = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
                let raw = base.saturating_mul(multiplier);
                if raw > *max {
                    *max
                } else {
                    raw
                }
            }
            Self::Linear { step, max } => {
                let raw = step.saturating_mul(attempt.saturating_add(1));
                if raw > *max {
                    *max
                } else {
                    raw
                }
            }
            Self::Fixed(d) => *d,
        }
    }
}

// ─── Retry policy ────────────────────────────────────────────────────────────

/// Full retry policy combining max attempts, backoff, and trigger conditions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (0 = no retries).
    pub max_retries: u32,
    /// Backoff strategy between attempts.
    pub backoff: BackoffStrategy,
    /// HTTP status codes that trigger a retry.
    pub retry_on_status: Vec<u16>,
    /// Whether to retry on network/timeout errors.
    pub retry_on_transport_error: bool,
}

impl RetryPolicy {
    /// Standard API policy: 3 retries, exponential 1 s → 30 s, retry on
    /// 429 / 502 / 503 / 504 and transport errors.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            max_retries: 3,
            backoff: BackoffStrategy::Exponential {
                base: Duration::from_secs(1),
                max: Duration::from_secs(30),
            },
            retry_on_status: vec![429, 502, 503, 504],
            retry_on_transport_error: true,
        }
    }

    /// Cautious policy for controlled/scraped sources: 2 retries,
    /// exponential 2 s → 60 s.
    #[must_use]
    pub fn cautious() -> Self {
        Self {
            max_retries: 2,
            backoff: BackoffStrategy::Exponential {
                base: Duration::from_secs(2),
                max: Duration::from_secs(60),
            },
            retry_on_status: vec![429, 503],
            retry_on_transport_error: true,
        }
    }

    /// Minimal policy for high-risk/onion sources: 1 retry, fixed 30 s.
    #[must_use]
    pub fn minimal() -> Self {
        Self {
            max_retries: 1,
            backoff: BackoffStrategy::Fixed(Duration::from_secs(30)),
            retry_on_status: vec![429, 503],
            retry_on_transport_error: true,
        }
    }

    /// No retries at all.
    #[must_use]
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            backoff: BackoffStrategy::Fixed(Duration::ZERO),
            retry_on_status: Vec::new(),
            retry_on_transport_error: false,
        }
    }

    /// Whether the given HTTP status code should trigger a retry.
    #[must_use]
    pub fn should_retry_status(&self, status: u16) -> bool {
        self.retry_on_status.contains(&status)
    }

    /// Whether retries remain for the given attempt number (zero-indexed).
    #[must_use]
    pub fn has_attempts_remaining(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

// ─── Retry decision ──────────────────────────────────────────────────────────

/// Outcome of evaluating whether to retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryDecision {
    /// Retry after the given delay.
    Retry(Duration),
    /// Do not retry.
    GiveUp,
}

/// Evaluate whether a request should be retried.
///
/// `attempt` is zero-indexed (0 = first retry after initial failure).
/// If the server sent a `Retry-After` header with a seconds value,
/// pass it as `retry_after_secs`; it overrides the computed backoff
/// when it is larger.
pub fn evaluate_retry(
    policy: &RetryPolicy,
    attempt: u32,
    retriable: bool,
    retry_after_secs: Option<u64>,
) -> RetryDecision {
    if !retriable || !policy.has_attempts_remaining(attempt) {
        return RetryDecision::GiveUp;
    }

    let mut delay = policy.backoff.delay_for(attempt);

    // Honour Retry-After if it demands a longer wait.
    if let Some(secs) = retry_after_secs {
        let server_delay = Duration::from_secs(secs);
        if server_delay > delay {
            delay = server_delay;
        }
    }

    // Apply jitter: ±25 % using cheap pseudo-randomness.
    delay = apply_jitter(delay);

    RetryDecision::Retry(delay)
}

/// Cheap pseudo-random jitter without pulling in the `rand` crate.
///
/// Uses the low-order nanoseconds of the system clock to add ±25 %
/// variation. This is not cryptographic; it only needs to spread
/// retries to avoid thundering herds.
fn apply_jitter(delay: Duration) -> Duration {
    let ms = delay.as_millis() as u64;
    if ms < 4 {
        return delay;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;

    let jitter_range = ms / 4; // 25 %
    let jitter_offset = nanos % (jitter_range * 2 + 1);
    let jittered = ms
        .saturating_sub(jitter_range)
        .saturating_add(jitter_offset);

    Duration::from_millis(jittered)
}

/// Parse a `Retry-After` header value into seconds.
///
/// Only handles the integer-seconds form (e.g. `"120"`). HTTP-date
/// values are ignored and return `None` — the caller falls back to
/// computed backoff in that case.
pub fn parse_retry_after(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_doubles() {
        let strat = BackoffStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
        };
        assert_eq!(strat.delay_for(0), Duration::from_secs(1));
        assert_eq!(strat.delay_for(1), Duration::from_secs(2));
        assert_eq!(strat.delay_for(2), Duration::from_secs(4));
        assert_eq!(strat.delay_for(3), Duration::from_secs(8));
    }

    #[test]
    fn exponential_backoff_caps_at_max() {
        let strat = BackoffStrategy::Exponential {
            base: Duration::from_secs(1),
            max: Duration::from_secs(10),
        };
        assert_eq!(strat.delay_for(10), Duration::from_secs(10));
    }

    #[test]
    fn linear_backoff() {
        let strat = BackoffStrategy::Linear {
            step: Duration::from_secs(5),
            max: Duration::from_secs(30),
        };
        assert_eq!(strat.delay_for(0), Duration::from_secs(5));
        assert_eq!(strat.delay_for(1), Duration::from_secs(10));
        assert_eq!(strat.delay_for(5), Duration::from_secs(30)); // capped
    }

    #[test]
    fn fixed_backoff() {
        let strat = BackoffStrategy::Fixed(Duration::from_secs(7));
        assert_eq!(strat.delay_for(0), Duration::from_secs(7));
        assert_eq!(strat.delay_for(99), Duration::from_secs(7));
    }

    #[test]
    fn retry_decision_give_up_when_no_retries_left() {
        let policy = RetryPolicy::standard();
        let decision = evaluate_retry(&policy, 3, true, None);
        assert_eq!(decision, RetryDecision::GiveUp);
    }

    #[test]
    fn retry_decision_give_up_when_not_retriable() {
        let policy = RetryPolicy::standard();
        let decision = evaluate_retry(&policy, 0, false, None);
        assert_eq!(decision, RetryDecision::GiveUp);
    }

    #[test]
    fn retry_decision_retries_when_possible() {
        let policy = RetryPolicy::standard();
        let decision = evaluate_retry(&policy, 0, true, None);
        assert!(matches!(decision, RetryDecision::Retry(_)));
    }

    #[test]
    fn retry_after_overrides_short_backoff() {
        let policy = RetryPolicy {
            max_retries: 1,
            backoff: BackoffStrategy::Fixed(Duration::from_secs(1)),
            retry_on_status: vec![429],
            retry_on_transport_error: false,
        };
        if let RetryDecision::Retry(delay) = evaluate_retry(&policy, 0, true, Some(60)) {
            // Should be at least 60 s (minus jitter at most 25%).
            assert!(delay.as_secs() >= 45);
        } else {
            panic!("expected Retry");
        }
    }

    #[test]
    fn parse_retry_after_integer() {
        assert_eq!(parse_retry_after("120"), Some(120));
        assert_eq!(parse_retry_after(" 30 "), Some(30));
    }

    #[test]
    fn parse_retry_after_http_date_returns_none() {
        assert_eq!(parse_retry_after("Fri, 31 Dec 2024 23:59:59 GMT"), None);
    }

    #[test]
    fn should_retry_status_check() {
        let policy = RetryPolicy::standard();
        assert!(policy.should_retry_status(429));
        assert!(policy.should_retry_status(503));
        assert!(!policy.should_retry_status(404));
        assert!(!policy.should_retry_status(200));
    }
}
