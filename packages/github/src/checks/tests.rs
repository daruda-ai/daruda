use std::time::Duration;

use crate::checks::{
    actions_job, check_run_status, entries, rollup_from_state, status_context_status, summarize,
};
use crate::{ActionsJob, CheckKind, CheckRollup, CheckRun, CheckStatus};

fn run(name: &str, status: CheckStatus, started: &str) -> CheckRun {
    CheckRun {
        kind: CheckKind::Run,
        workflow: Some("CI".into()),
        event: None,
        name: name.into(),
        status,
        details_url: None,
        started_at: Some(started.into()),
        completed_at: None,
    }
}

fn queued(name: &str) -> CheckRun {
    CheckRun {
        started_at: None,
        ..run(name, CheckStatus::Pending, "")
    }
}

fn in_workflow(run: CheckRun, workflow: &str) -> CheckRun {
    CheckRun {
        workflow: Some(workflow.into()),
        ..run
    }
}

fn on_event(run: CheckRun, event: &str) -> CheckRun {
    CheckRun {
        event: Some(event.into()),
        ..run
    }
}

fn status(run: CheckRun) -> CheckRun {
    CheckRun {
        kind: CheckKind::Status,
        workflow: None,
        event: None,
        ..run
    }
}

#[test]
fn github_values_map_to_what_a_check_counts_as() {
    use CheckStatus::*;
    let runs = [
        ("COMPLETED", Some("SUCCESS"), Success),
        ("COMPLETED", Some("FAILURE"), Failure),
        ("COMPLETED", Some("CANCELLED"), Failure),
        ("COMPLETED", Some("TIMED_OUT"), Failure),
        ("COMPLETED", Some("ACTION_REQUIRED"), Failure),
        ("COMPLETED", Some("STARTUP_FAILURE"), Failure),
        ("COMPLETED", Some("SKIPPED"), Ignored),
        ("COMPLETED", Some("NEUTRAL"), Ignored),
        ("COMPLETED", Some("STALE"), Ignored),
        ("COMPLETED", None, Ignored),
        ("IN_PROGRESS", None, Pending),
        ("QUEUED", None, Pending),
    ];
    for (status, conclusion, expected) in runs {
        assert_eq!(
            check_run_status(status, conclusion),
            expected,
            "{status} {conclusion:?}"
        );
    }
    let contexts = [
        ("SUCCESS", Success),
        ("FAILURE", Failure),
        ("ERROR", Failure),
        ("PENDING", Pending),
        ("EXPECTED", Pending),
    ];
    for (state, expected) in contexts {
        assert_eq!(status_context_status(state), expected, "{state}");
    }
}

#[test]
fn checks_add_up_by_the_one_rule() {
    use CheckStatus::*;
    let cases: Vec<(&str, Vec<CheckRun>, CheckRollup)> = vec![
        ("no checks", vec![], CheckRollup::None),
        (
            "only skipped",
            vec![run("a", Ignored, "1")],
            CheckRollup::None,
        ),
        (
            "all passing",
            vec![run("a", Success, "1"), run("b", Success, "1")],
            CheckRollup::Passing,
        ),
        (
            "skipped does not hide a pass",
            vec![run("a", Success, "1"), run("b", Ignored, "1")],
            CheckRollup::Passing,
        ),
        (
            "one failure",
            vec![run("a", Success, "1"), run("b", Failure, "1")],
            CheckRollup::Failing { failed: Some(1) },
        ),
        (
            "a failure outranks pending",
            vec![run("a", Pending, "1"), run("b", Failure, "1")],
            CheckRollup::Failing { failed: Some(1) },
        ),
        (
            "pending without failure",
            vec![run("a", Pending, "1"), run("b", Success, "1")],
            CheckRollup::Pending,
        ),
        (
            "a re-run that passed replaces the failure",
            vec![run("a", Failure, "1"), run("a", Success, "2")],
            CheckRollup::Passing,
        ),
        (
            "a re-run that failed replaces the pass, whatever the order",
            vec![run("a", Failure, "2"), run("a", Success, "1")],
            CheckRollup::Failing { failed: Some(1) },
        ),
        (
            "a queued re-run replaces the failure it re-runs",
            vec![run("a", Failure, "1"), queued("a")],
            CheckRollup::Pending,
        ),
        (
            "the same job name in two workflows is two checks",
            vec![
                run("build", Failure, "1"),
                in_workflow(run("build", Success, "2"), "Release"),
            ],
            CheckRollup::Failing { failed: Some(1) },
        ),
        (
            "a push run's failure is not hidden by the pull_request run after it",
            vec![
                on_event(run("test", Failure, "1"), "push"),
                on_event(run("test", Success, "2"), "pull_request"),
            ],
            CheckRollup::Failing { failed: Some(1) },
        ),
        (
            "a re-run of one event still replaces that event's earlier run",
            vec![
                on_event(run("test", Failure, "1"), "pull_request"),
                on_event(run("test", Success, "2"), "pull_request"),
            ],
            CheckRollup::Passing,
        ),
        (
            "a status context does not replace a check run of the same name",
            vec![run("lint", Failure, "1"), status(run("lint", Success, "2"))],
            CheckRollup::Failing { failed: Some(1) },
        ),
    ];
    for (label, runs, expected) in cases {
        assert_eq!(summarize(&runs).0, expected, "{label}");
    }
}

#[test]
fn failures_carry_their_actions_job_in_input_order() {
    let mut first = run("build", CheckStatus::Failure, "1");
    first.details_url =
        Some("https://github.com/cli/cli/actions/runs/41720844033/job/126212878432".into());
    let mut second = run("lint", CheckStatus::Failure, "1");
    second.details_url = Some("https://ci.example.com/build/7".into());
    let (_, failed) = summarize(&[first, second]);
    let names: Vec<_> = failed.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["build", "lint"]);
    assert_eq!(
        failed[0].job,
        Some(ActionsJob {
            run: 41_720_844_033,
            job: 126_212_878_432
        })
    );
    assert_eq!(failed[1].job, None, "external CI has no Actions job");
}

#[test]
fn only_an_actions_job_url_yields_a_job() {
    let cases = [
        (
            "actions job",
            "https://github.com/o/r/actions/runs/1/job/2",
            Some(ActionsJob { run: 1, job: 2 }),
        ),
        (
            "query string",
            "https://github.com/o/r/actions/runs/1/job/2?pr=5",
            Some(ActionsJob { run: 1, job: 2 }),
        ),
        (
            "run without job",
            "https://github.com/o/r/actions/runs/1",
            None,
        ),
        (
            "not numeric",
            "https://github.com/o/r/actions/runs/x/job/2",
            None,
        ),
        ("other path", "https://github.com/o/r/runs/1/job/2", None),
        ("external", "https://ci.example.com/build/7", None),
    ];
    for (label, url, expected) in cases {
        assert_eq!(actions_job(url), expected, "{label}");
    }
}

#[test]
fn githubs_own_rollup_state_maps_without_a_count() {
    assert_eq!(rollup_from_state("SUCCESS"), CheckRollup::Passing);
    assert_eq!(
        rollup_from_state("FAILURE"),
        CheckRollup::Failing { failed: None }
    );
    assert_eq!(rollup_from_state("PENDING"), CheckRollup::Pending);
    assert_eq!(rollup_from_state(""), CheckRollup::None);
}

#[test]
fn an_incomplete_run_is_pending_whatever_its_conclusion() {
    assert_eq!(
        check_run_status("IN_PROGRESS", Some("FAILURE")),
        CheckStatus::Pending
    );
}

#[test]
fn the_check_list_reads_failing_running_passing_skipped() {
    use CheckStatus::*;
    let runs = [
        run("pass-1", Success, "1"),
        run("skip", Ignored, "1"),
        run("run-1", Pending, "1"),
        run("fail-1", Failure, "1"),
        run("pass-2", Success, "1"),
        status(run("fail-2", Failure, "1")),
        queued("run-2"),
    ];
    let names: Vec<String> = entries(&runs).into_iter().map(|e| e.run.name).collect();
    assert_eq!(
        names,
        [
            "fail-1", "fail-2", "run-1", "run-2", "pass-1", "pass-2", "skip"
        ],
        "by status, GitHub's order within each"
    );
}

#[test]
fn the_check_list_and_the_rollup_count_the_same_latest_runs() {
    use CheckStatus::*;
    // A rerun that passed replaces the failure it reran, in both.
    let runs = [run("test", Failure, "1"), run("test", Success, "2")];
    let listed = entries(&runs);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].run.status, Success);
    assert_eq!(summarize(&runs).0, CheckRollup::Passing);
}

#[test]
fn an_entry_carries_its_job_and_how_long_it_took() {
    let at = |started: &str, completed: Option<&str>| CheckRun {
        details_url: Some("https://github.com/o/r/actions/runs/1/job/2".into()),
        completed_at: completed.map(Into::into),
        ..run("build", CheckStatus::Success, started)
    };
    let cases = [
        (
            "completed",
            at("2026-11-09T23:21:50Z", Some("2026-11-09T23:22:57Z")),
            Some(Duration::from_secs(67)),
        ),
        ("still running", at("2026-11-09T23:21:50Z", None), None),
        (
            "completed before it started, as skipped runs report",
            at("2026-11-09T23:21:55Z", Some("2026-11-09T23:21:46Z")),
            None,
        ),
        (
            "malformed",
            at("yesterday", Some("2026-11-09T23:21:46Z")),
            None,
        ),
    ];
    for (label, check, expected) in cases {
        let entry = entries(&[check]).remove(0);
        assert_eq!(entry.duration, expected, "{label}");
        assert_eq!(entry.job, Some(ActionsJob { run: 1, job: 2 }), "{label}");
    }
}
