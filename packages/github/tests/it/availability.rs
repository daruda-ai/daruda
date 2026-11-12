use std::time::Duration;

use daruda_github::availability::{BackoffReason, backoff_delay};
use daruda_github::{FailureKind, GhError};

#[test]
fn backoff_doubles_from_a_minute_to_half_an_hour() {
    let minutes = |n, reason| backoff_delay(n, reason).as_secs() / 60;
    let transient: Vec<u64> = (1..=7)
        .map(|n| minutes(n, BackoffReason::Transient))
        .collect();
    assert_eq!(transient, [1, 2, 4, 8, 16, 30, 30]);
    let limited: Vec<u64> = (1..=5)
        .map(|n| minutes(n, BackoffReason::RateLimited))
        .collect();
    assert_eq!(limited, [5, 5, 5, 8, 16]);
    assert_eq!(
        backoff_delay(u32::MAX, BackoffReason::Transient),
        Duration::from_secs(30 * 60)
    );
}

#[test]
fn failures_are_classified_for_the_poller() {
    let exit = |code, stderr: &str| GhError::Exit {
        code: Some(code),
        stderr: stderr.into(),
    };
    let cases = [
        ("gh missing", GhError::NotFound, FailureKind::Missing),
        ("no login", exit(4, "gh auth login"), FailureKind::LoggedOut),
        (
            "rate limit",
            exit(1, "HTTP 403: API rate limit exceeded for user"),
            FailureKind::RateLimited,
        ),
        (
            "network",
            exit(1, "error connecting to api.github.com"),
            FailureKind::Transient,
        ),
        (
            "timeout",
            GhError::Timeout(Duration::from_secs(30)),
            FailureKind::Transient,
        ),
    ];
    for (label, error, expected) in cases {
        assert_eq!(error.kind(), expected, "{label}");
    }
}
