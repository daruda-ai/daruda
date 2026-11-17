//! A spent rate limit on the Pull requests page: the pause waits for the
//! reset GitHub reports, asked once per pause, and says whether its end is
//! reported or guessed.

use std::time::Duration;

use daruda_github::availability::Resume;
use daruda_github::fake::{FakeGh, GhFailure, GhOp};
use gpui::TestAppContext;

use super::GitFixture;
use super::pull_request_page::{linked_window, show_page, trouble};
use crate::workspace::pages::pull_requests::list::Trouble;

fn hit_the_limit(f: &GitFixture, gh: &FakeGh, cx: &mut TestAppContext) {
    gh.fail(
        GhOp::SearchPrsPage,
        GhFailure::Exit {
            code: Some(1),
            stderr: "gh: API rate limit exceeded for user ID 1. (HTTP 403)".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
}

/// How the group's pause ends, and how long from now.
fn pause(f: &GitFixture, cx: &mut TestAppContext) -> (Resume, Duration) {
    let now = cx.executor().now();
    match trouble(f, cx) {
        Some(Trouble::RateLimited { until, resume }) => (resume, until.duration_since(now)),
        other => panic!("expected a rate-limit pause, got {other:?}"),
    }
}

#[gpui::test]
fn a_rate_limit_waits_for_the_reset_github_reports(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    gh.set_rate_limit_reset("github.com", chrono::Utc::now().timestamp() + 40 * 60);
    hit_the_limit(&f, &gh, cx);

    assert_eq!(
        gh.count(GhOp::RateLimitReset),
        1,
        "every tab heard the limit; the reset is asked once"
    );
    let (resume, wait) = pause(&f, cx);
    assert_eq!(resume, Resume::Reported);
    assert!(
        (Duration::from_secs(39 * 60)..=Duration::from_secs(41 * 60)).contains(&wait),
        "the pause ends at the reported reset, not the five-minute guess: {wait:?}"
    );
}

#[gpui::test]
fn without_a_reported_reset_the_guess_stands_and_says_so(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    hit_the_limit(&f, &gh, cx);

    assert_eq!(gh.count(GhOp::RateLimitReset), 1);
    let (resume, wait) = pause(&f, cx);
    assert_eq!(resume, Resume::Estimated);
    assert!(wait <= Duration::from_secs(5 * 60), "{wait:?}");
}
