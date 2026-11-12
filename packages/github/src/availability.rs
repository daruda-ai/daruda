//! Whether `gh` can be asked right now, and how long to wait after failures.

use std::time::{Duration, Instant};

/// What `gh auth status` says about one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhAuth {
    Missing,
    LoggedOut,
    Ready,
}

/// Why requests are paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffReason {
    RateLimited,
    Transient,
}

/// The poller's view of `gh` for one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhAvailability {
    /// Not asked yet.
    Unknown,
    Missing,
    LoggedOut,
    Ready,
    /// Requests failed; none are sent before `until`.
    Backoff {
        until: Instant,
        reason: BackoffReason,
    },
}

impl From<GhAuth> for GhAvailability {
    fn from(auth: GhAuth) -> Self {
        match auth {
            GhAuth::Missing => Self::Missing,
            GhAuth::LoggedOut => Self::LoggedOut,
            GhAuth::Ready => Self::Ready,
        }
    }
}

const FIRST_DELAY: Duration = Duration::from_secs(60);
/// A rate limit resets on the hour at the latest; gh does not relay when.
const RATE_LIMIT_FLOOR: Duration = Duration::from_secs(5 * 60);
const MAX_DELAY: Duration = Duration::from_secs(30 * 60);

/// The pause after the `failures`-th consecutive failure (1-based): one
/// minute, doubling to thirty; a rate limit waits at least five.
pub fn backoff_delay(failures: u32, reason: BackoffReason) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    let delay = FIRST_DELAY.saturating_mul(1 << doublings).min(MAX_DELAY);
    match reason {
        BackoffReason::RateLimited => delay.max(RATE_LIMIT_FLOOR),
        BackoffReason::Transient => delay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_failures_reads_as_the_first() {
        assert_eq!(backoff_delay(0, BackoffReason::Transient), FIRST_DELAY);
    }
}
