//! `failed_log.txt` is a real `gh run view --job --log-failed` output
//! (`cli/cli`, gh 2.74.2): its first line and the `go test` step through
//! post-job cleanup, ANSI escapes and BOM included.

use daruda_github::prompt::{CheckLog, clean_log, failed_checks, strip_control};
use daruda_github::{CheckRollup, FailedCheck, PrState, PrSummary};

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
        checks: CheckRollup::Failing { failed: 1 },
        failed: Vec::new(),
        review: None,
        updated_at: String::new(),
    }
}

fn check(name: &str) -> FailedCheck {
    FailedCheck {
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
    ];
    for (label, input, expected) in cases {
        assert_eq!(strip_control(input), expected, "{label}");
    }
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
                    name: "pull-kubernetes-e2e".into(),
                    details_url: Some("https://prow.k8s.io/view/1".into()),
                    job: None,
                },
                log: Err("not a GitHub Actions check".into()),
            },
        ],
    );
    assert!(prompt.starts_with("Pull request #14927 (Multi-account draft) has failing CI checks."));
    assert!(prompt.contains("## build (windows-latest)\nDetails: https://github.com/cli/cli/actions/runs/1/job/2\n```\n##[group]Run go test"));
    assert!(prompt.contains("## pull-kubernetes-e2e\nDetails: https://prow.k8s.io/view/1\n(no log: not a GitHub Actions check)"));
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
