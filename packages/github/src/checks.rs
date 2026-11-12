//! The one place that decides what a pull request's checks add up to.
//!
//! Cancelled and timed-out runs count as failures; skipped, neutral and stale
//! ones are left out; a check re-run under the same name counts only its
//! latest run.

use std::collections::HashMap;

use crate::{ActionsJob, CheckRollup, CheckRun, CheckStatus, FailedCheck};

/// A check run's `status` and `conclusion`, as GitHub's GraphQL spells them.
pub fn check_run_status(status: &str, conclusion: Option<&str>) -> CheckStatus {
    if status != "COMPLETED" {
        return CheckStatus::Pending;
    }
    match conclusion {
        Some("SUCCESS") => CheckStatus::Success,
        Some("FAILURE" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE") => {
            CheckStatus::Failure
        }
        _ => CheckStatus::Ignored,
    }
}

/// A commit status context's `state` (external CI that is not a check run).
pub fn status_context_status(state: &str) -> CheckStatus {
    match state {
        "SUCCESS" => CheckStatus::Success,
        "FAILURE" | "ERROR" => CheckStatus::Failure,
        "PENDING" | "EXPECTED" => CheckStatus::Pending,
        _ => CheckStatus::Ignored,
    }
}

/// GitHub's own rollup `state`, used when the check list was truncated and
/// the runs in hand cannot be trusted to add up.
pub fn rollup_from_state(state: &str) -> CheckRollup {
    match state {
        "SUCCESS" => CheckRollup::Passing,
        "FAILURE" | "ERROR" => CheckRollup::Failing { failed: 0 },
        "PENDING" | "EXPECTED" => CheckRollup::Pending,
        _ => CheckRollup::None,
    }
}

/// Add `runs` up, returning the rollup and the failing checks behind it.
pub fn summarize(runs: &[CheckRun]) -> (CheckRollup, Vec<FailedCheck>) {
    let mut latest: HashMap<&str, &CheckRun> = HashMap::new();
    for run in runs {
        latest
            .entry(run.name.as_str())
            .and_modify(|kept| {
                if run.started_at > kept.started_at {
                    *kept = run;
                }
            })
            .or_insert(run);
    }
    // Input order, so the failures read in the order GitHub listed them.
    let counted: Vec<&CheckRun> = runs
        .iter()
        .filter(|run| std::ptr::eq(latest[run.name.as_str()], *run))
        .filter(|run| run.status != CheckStatus::Ignored)
        .collect();
    let failed: Vec<FailedCheck> = counted
        .iter()
        .filter(|run| run.status == CheckStatus::Failure)
        .map(|run| FailedCheck {
            name: run.name.clone(),
            details_url: run.details_url.clone(),
            job: run.details_url.as_deref().and_then(actions_job),
        })
        .collect();
    let rollup = if counted.is_empty() {
        CheckRollup::None
    } else if !failed.is_empty() {
        CheckRollup::Failing {
            failed: u32::try_from(failed.len()).unwrap_or(u32::MAX),
        }
    } else if counted.iter().any(|run| run.status == CheckStatus::Pending) {
        CheckRollup::Pending
    } else {
        CheckRollup::Passing
    };
    (rollup, failed)
}

/// The Actions job a check's details URL points at:
/// `https://<host>/<owner>/<repo>/actions/runs/<run>/job/<job>`.
pub fn actions_job(details_url: &str) -> Option<ActionsJob> {
    let path = details_url.split(['?', '#']).next()?;
    let mut parts = path.rsplit('/');
    let job = parts.next()?.parse().ok()?;
    if parts.next()? != "job" {
        return None;
    }
    let run = parts.next()?.parse().ok()?;
    if parts.next()? != "runs" || parts.next()? != "actions" {
        return None;
    }
    Some(ActionsJob { run, job })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_incomplete_run_is_pending_whatever_its_conclusion() {
        assert_eq!(
            check_run_status("IN_PROGRESS", Some("FAILURE")),
            CheckStatus::Pending
        );
    }
}
