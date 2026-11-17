//! Whether `gh` can be asked right now, and how long to wait after failures.

use std::time::{Duration, Instant};

/// What `gh auth status` says about one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhAuth {
    Missing,
    LoggedOut,
    Ready,
}

/// Where `gh` keeps the token it uses for a host, as `gh auth status`
/// names it. Never the token itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    /// The system credential store.
    Keyring,
    /// An environment variable, such as `GH_TOKEN`, which overrides any
    /// stored login.
    Env(String),
    /// gh's own `hosts.yml`.
    ConfigFile,
    /// A source this crate does not know, as gh worded it.
    Other(String),
}

impl CredentialSource {
    fn parse(source: &str) -> Self {
        if source == "keyring" {
            Self::Keyring
        } else if source.contains(['/', '\\']) {
            Self::ConfigFile
        } else if !source.is_empty()
            && source
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            Self::Env(source.to_owned())
        } else {
            Self::Other(source.to_owned())
        }
    }
}

/// The account `gh` acts as on a host, and where its token comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhAccount {
    pub login: String,
    pub source: CredentialSource,
}

/// What `gh auth status` says about one host: whether it can be asked,
/// and as whom when it can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthStatus {
    pub auth: GhAuth,
    /// The active account; `None` when gh is not ready or its output did
    /// not name one.
    pub account: Option<GhAccount>,
}

impl From<GhAuth> for AuthStatus {
    fn from(auth: GhAuth) -> Self {
        Self {
            auth,
            account: None,
        }
    }
}

/// The active account on `host` in `gh auth status` output (stdout and
/// stderr together: gh writes to either). Measured with gh 2.74.2:
///
/// ```text
///   ✓ Logged in to github.com account octocat (keyring)
///   - Active account: true
/// ```
pub fn parse_active_account(output: &str, host: &str) -> Option<GhAccount> {
    let prefix = format!("Logged in to {host} account ");
    let mut candidate: Option<GhAccount> = None;
    for line in output.lines().map(str::trim) {
        if let Some(rest) = line.split_once(&prefix).map(|(_, rest)| rest) {
            candidate = rest.split_once(" (").and_then(|(login, source)| {
                let source = source.strip_suffix(')')?;
                (!login.is_empty() && !login.contains(char::is_whitespace)).then(|| GhAccount {
                    login: login.to_owned(),
                    source: CredentialSource::parse(source),
                })
            });
        } else if line.starts_with("X ") || line.starts_with("✓ ") {
            candidate = None;
        } else if line == "- Active account: true" && candidate.is_some() {
            return candidate;
        }
    }
    None
}

/// Why requests are paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffReason {
    RateLimited,
    Transient,
}

/// Where a pause's end comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    /// The poller's own backoff: a guess at when asking makes sense again.
    Estimated,
    /// GitHub reported when the spent rate limit resets.
    Reported,
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
        resume: Resume,
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

/// Slack after a reported reset, so the first request does not land on the
/// edge of the window GitHub resets.
const RESET_GRACE: Duration = Duration::from_secs(1);
/// A reported reset further out than this is not trusted: a primary limit
/// resets within the hour.
const REPORTED_RESET_MAX: Duration = Duration::from_secs(2 * 60 * 60);

/// When to ask again after GitHub reported that a spent limit resets at
/// `reset` (Unix seconds), read at `wall_now` (Unix seconds) and `now` on
/// the poller's clock. `None` for a reset already past or implausibly far,
/// leaving the estimated pause in place.
pub fn reported_resume(reset: i64, wall_now: i64, now: Instant) -> Option<Instant> {
    let ahead = u64::try_from(reset.checked_sub(wall_now)?)
        .ok()
        .filter(|seconds| *seconds > 0)?;
    let ahead = Duration::from_secs(ahead);
    (ahead <= REPORTED_RESET_MAX).then(|| now + ahead + RESET_GRACE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_failures_reads_as_the_first() {
        assert_eq!(backoff_delay(0, BackoffReason::Transient), FIRST_DELAY);
    }
}
