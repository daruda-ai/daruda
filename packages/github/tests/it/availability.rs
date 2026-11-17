use std::time::Duration;

use std::time::Instant;

use daruda_github::availability::{
    BackoffReason, CredentialSource, GhAccount, backoff_delay, parse_active_account,
    reported_resume,
};
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
            "revoked token",
            exit(1, "gh: Bad credentials (HTTP 401)"),
            FailureKind::LoggedOut,
        ),
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

#[test]
fn the_active_account_and_its_source_are_read_never_the_token() {
    let output = "github.com\n  \
        ✓ Logged in to github.com account other (keyring)\n  \
        - Active account: false\n  \
        - Token: gho_************\n  \
        ✓ Logged in to github.com account octocat (GH_TOKEN)\n  \
        - Active account: true\n  \
        - Token: gho_************\n";
    assert_eq!(
        parse_active_account(output, "github.com"),
        Some(GhAccount {
            login: "octocat".into(),
            source: CredentialSource::Env("GH_TOKEN".into()),
        }),
        "the inactive account before it is skipped"
    );
    // (label, source as gh names it, parsed)
    let sources = [
        ("keychain", "keyring", CredentialSource::Keyring),
        (
            "env",
            "GITHUB_TOKEN",
            CredentialSource::Env("GITHUB_TOKEN".into()),
        ),
        (
            "config file",
            "/home/u/.config/gh/hosts.yml",
            CredentialSource::ConfigFile,
        ),
        (
            "unknown",
            "oauth_token",
            CredentialSource::Other("oauth_token".into()),
        ),
    ];
    for (label, source, parsed) in sources {
        let line = format!("  ✓ Logged in to h account a ({source})\n  - Active account: true");
        assert_eq!(
            parse_active_account(&line, "h").map(|account| account.source),
            Some(parsed),
            "{label}"
        );
    }
    // (label, output)
    let none = [
        (
            "a failed login names no account",
            "  X Failed to log in to github.com using token (GH_TOKEN)\n  - Active account: true",
        ),
        (
            "another host's account",
            "  ✓ Logged in to ghe.corp account a (keyring)\n  - Active account: true",
        ),
        ("no output", ""),
    ];
    for (label, output) in none {
        assert_eq!(parse_active_account(output, "github.com"), None, "{label}");
    }
}

#[test]
fn a_reported_reset_is_trusted_only_when_it_is_soon() {
    let now = Instant::now();
    let wall = 1_800_000_000;
    // (label, reset, expected wait from now)
    let cases = [
        ("ten minutes ahead", wall + 600, Some(601)),
        ("at the cap", wall + 7200, Some(7201)),
        ("beyond the cap", wall + 7201, None),
        ("already reset", wall, None),
        ("in the past", wall - 30, None),
    ];
    for (label, reset, expected) in cases {
        let wait =
            reported_resume(reset, wall, now).map(|until| until.duration_since(now).as_secs());
        assert_eq!(wait, expected, "{label}");
    }
}
