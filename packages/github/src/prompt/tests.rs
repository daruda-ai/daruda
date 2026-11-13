//! `failed_log.txt` is a real `gh run view --job --log-failed` output
//! (`cli/cli`, gh 2.74.2): its first line and the `go test` step through
//! post-job cleanup, ANSI escapes and BOM included.

use super::*;
use crate::prompt::{CheckLog, NoLog, clean_log, failed_checks, one_line, strip_control};
use crate::{CheckRollup, FailedCheck, PrState, PrSummary};

fn fixture() -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/failed_log.txt");
    std::fs::read_to_string(path).unwrap()
}

fn pr() -> PrSummary {
    PrSummary {
        number: 14927,
        url: String::new(),
        title: "Multi-account draft".into(),
        state: PrState::Open,
        head_branch: "feat".into(),
        cross_repo: false,
        checks: CheckRollup::Failing { failed: Some(1) },
        failed: Vec::new(),
        review: None,
        updated_at: String::new(),
    }
}

fn check(name: &str) -> FailedCheck {
    FailedCheck {
        workflow: None,
        event: None,
        name: name.into(),
        details_url: Some("https://github.com/cli/cli/actions/runs/1/job/2".into()),
        job: None,
    }
}

#[test]
fn control_characters_cannot_reach_a_terminal() {
    let cases = [
        ("csi color", "\u{1b}[36;1mgo test\u{1b}[0m", "go test"),
        ("paste end marker", "a\u{1b}[201~b", "ab"),
        ("osc title", "\u{1b}]0;title\u{7}x", "x"),
        ("carriage return and bell", "a\r\u{7}b", "ab"),
        ("bom", "\u{feff}x", "x"),
        ("newline and tab stay", "a\n\tb", "a\n\tb"),
        ("unicode stays", "한글 ✓", "한글 ✓"),
        (
            "a cut-short csi keeps what follows",
            "a\u{1b}[31\nb",
            "a\nb",
        ),
    ];
    for (label, input, expected) in cases {
        assert_eq!(strip_control(input), expected, "{label}");
    }
}

#[test]
fn one_line_fields_cannot_start_a_line() {
    assert_eq!(one_line("build\n## injected\tx"), "build ## injected x");
}

#[test]
fn a_real_log_is_cut_to_the_step_that_failed() {
    let log = clean_log(&fixture());
    assert!(log.starts_with("##[group]Run go test"), "{}", &log[..80]);
    assert!(
        log.contains("--- FAIL: TestResetExperimentDetachesIntegrationWithoutDeletingAccounts")
    );
    assert!(
        log.trim_end()
            .ends_with("##[error]Process completed with exit code 1.")
    );
    assert!(!log.contains("Post job cleanup"), "cleanup is dropped");
    assert!(
        !log.contains("UNKNOWN STEP"),
        "the job/step prefix is dropped"
    );
    assert!(!log.contains("2026-11-03T"), "timestamps are dropped");
    assert!(!log.contains('\u{1b}'));
    assert!(log.chars().count() <= 9_100, "elided to head + tail");
}

#[test]
fn without_an_error_marker_the_last_step_is_kept() {
    let raw = "j\ts\t2026-01-01T00:00:00.0000000Z ##[group]Run setup\n\
               j\ts\t2026-01-01T00:00:00.0000000Z ok\n\
               j\ts\t2026-01-01T00:00:00.0000000Z ##[group]Run test\n\
               j\ts\t2026-01-01T00:00:00.0000000Z killed";
    assert_eq!(clean_log(raw), "##[group]Run test\nkilled");
}

#[test]
fn a_log_without_an_error_marker_is_kept_whole() {
    let raw =
        "job\tstep\t2026-01-01T00:00:00.0000000Z one\njob\tstep\t2026-01-01T00:00:00.0000000Z two";
    assert_eq!(clean_log(raw), "one\ntwo");
}

#[test]
fn the_prompt_quotes_each_check_and_says_when_a_log_is_missing() {
    let prompt = failed_checks(
        &pr(),
        &[
            CheckLog {
                check: check("build (windows-latest)"),
                log: Ok(fixture()),
            },
            CheckLog {
                check: FailedCheck {
                    workflow: None,
                    event: None,
                    name: "pull-kubernetes-e2e".into(),
                    details_url: Some("https://prow.k8s.io/view/1".into()),
                    job: None,
                },
                log: Err(NoLog::NotActions),
            },
        ],
    );
    assert!(prompt.starts_with("Pull request #14927 (Multi-account draft) has failing CI checks."));
    assert!(prompt.contains("## build (windows-latest)\nDetails: https://github.com/cli/cli/actions/runs/1/job/2\n```\n##[group]Run go test"));
    assert!(prompt.contains("## pull-kubernetes-e2e\nDetails: https://prow.k8s.io/view/1\n(no log: not a GitHub Actions check)"));
}

#[test]
fn many_long_failures_stay_within_the_budget() {
    let log = fixture();
    let logs: Vec<CheckLog> = (0..40)
        .map(|i| CheckLog {
            check: check(&format!("job {i}")),
            log: Ok(log.clone()),
        })
        .collect();
    let prompt = failed_checks(&pr(), &logs);
    assert!(
        prompt.chars().count() <= 30_000,
        "{}",
        prompt.chars().count()
    );
    assert!(
        prompt.contains("more failing checks omitted"),
        "the rest are counted"
    );
}

#[test]
fn same_named_jobs_read_apart_by_workflow() {
    let mut ci = check("build");
    ci.workflow = Some("CI".into());
    ci.event = Some("push".into());
    let prompt = failed_checks(
        &pr(),
        &[CheckLog {
            check: ci,
            log: Err(NoLog::Unreadable("x".into())),
        }],
    );
    assert!(prompt.contains("## CI / build (push)\n"), "{prompt}");
}

#[test]
fn a_log_with_fences_gets_a_longer_one() {
    let log = "job\tstep\t2026-01-01T00:00:00.0000000Z ```rust\n##[error]boom";
    let prompt = failed_checks(
        &pr(),
        &[CheckLog {
            check: check("lint"),
            log: Ok(log.into()),
        }],
    );
    assert!(
        prompt.contains("````\n```rust\n##[error]boom\n````"),
        "{prompt}"
    );
}

#[test]
fn a_short_text_is_not_elided() {
    assert_eq!(elide_middle("abc", 1, 2), "abc");
    assert_eq!(elide_middle("abcdef", 1, 2), format!("a\n{ELIDED}\nef"));
}

#[test]
fn the_fence_outgrows_the_quoted_backticks() {
    assert_eq!(fence_for("plain"), "```");
    assert_eq!(fence_for("a ```` b"), "`````");
}

mod review;
