//! Retry policy for transient errors before opening an LLM response stream.
//!
//! Uses `is_retryable` on `LlmClient::stream` open errors to manage retries,
//! backoff, and cancellation together. Failures after SSE reading begins
//! are excluded.

use std::time::Duration;

/// Policy combining exponential backoff, jitter, and a cumulative timeout.
///
/// `Default` returns fixed values shared across agen. Extend this if caller
/// overrides become necessary.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Exponential base. Full jitter samples the actual wait from an upper bound
    /// of `base * 2^attempt`, capped at `cap`.
    pub base: Duration,
    /// Maximum wait per attempt.
    pub cap: Duration,
    /// Total attempts (initial attempt + retries). `1` disables retries.
    pub max_attempts: u32,
    /// Cumulative timeout from the first send. Waits exceeding it are cut short.
    pub total_timeout: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(500),
            cap: Duration::from_secs(10),
            max_attempts: 4,
            total_timeout: Duration::from_secs(40),
        }
    }
}

impl RetryPolicy {
    /// Return the wait after failure number `attempt` (zero-indexed).
    /// For a `Retry-After` override, use its value directly instead of calling this.
    pub fn backoff(&self, attempt: u32) -> Duration {
        let shift = attempt.min(20);
        let base_nanos = self.base.as_nanos() as u64;
        let exp_nanos = base_nanos.saturating_mul(1u64 << shift);
        let cap_nanos = self.cap.as_nanos() as u64;
        let upper = exp_nanos.min(cap_nanos);
        Duration::from_nanos(jitter_nanos(upper))
    }
}

/// Sample a pseudorandom value from `[0, max_nanos]`. This lightweight
/// implementation mixes the low bits of `SystemTime` with splitmix64.
/// It is not cryptographically random, but suffices to avoid full-jitter collisions.
fn jitter_nanos(max_nanos: u64) -> u64 {
    if max_nanos == 0 {
        return 0;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x % (max_nanos + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_values() {
        let p = RetryPolicy::default();
        assert_eq!(p.base, Duration::from_millis(500));
        assert_eq!(p.cap, Duration::from_secs(10));
        assert_eq!(p.max_attempts, 4);
        assert_eq!(p.total_timeout, Duration::from_secs(40));
    }

    #[test]
    fn backoff_respects_cap() {
        let p = RetryPolicy::default();
        for attempt in 0..30u32 {
            assert!(
                p.backoff(attempt) <= p.cap,
                "attempt {attempt} exceeded cap",
            );
        }
    }

    #[test]
    fn backoff_zero_when_base_zero() {
        let p = RetryPolicy {
            base: Duration::ZERO,
            cap: Duration::from_secs(10),
            max_attempts: 4,
            total_timeout: Duration::from_secs(30),
        };
        for attempt in 0..5 {
            assert_eq!(p.backoff(attempt), Duration::ZERO);
        }
    }
}
