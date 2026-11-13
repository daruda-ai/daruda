//! The one place that decides what a pull request's checks add up to.
//!
//! Cancelled and timed-out runs count as failures; skipped, neutral and stale
//! ones are left out; a check re-run counts only its latest run, a queued
//! re-run included. A check is identified by its kind, workflow, triggering
//! event and name, as GitHub lists them (`CI / test (push)`).

use std::collections::HashMap;
use std::time::Duration;

use crate::{ActionsJob, CheckEntry, CheckKind, CheckRollup, CheckRun, CheckStatus, FailedCheck};

/// A check run's `status` and `conclusion`, as GitHub's GraphQL spells them.
pub(crate) fn check_run_status(status: &str, conclusion: Option<&str>) -> CheckStatus {
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
pub(crate) fn status_context_status(state: &str) -> CheckStatus {
    match state {
        "SUCCESS" => CheckStatus::Success,
        "FAILURE" | "ERROR" => CheckStatus::Failure,
        "PENDING" | "EXPECTED" => CheckStatus::Pending,
        _ => CheckStatus::Ignored,
    }
}

/// GitHub's own rollup `state`, used when the check list was truncated and
/// the runs in hand cannot be trusted to add up.
pub(crate) fn rollup_from_state(state: &str) -> CheckRollup {
    match state {
        "SUCCESS" => CheckRollup::Passing,
        "FAILURE" | "ERROR" => CheckRollup::Failing { failed: None },
        "PENDING" | "EXPECTED" => CheckRollup::Pending,
        _ => CheckRollup::None,
    }
}

/// Add `runs` up, returning the rollup and the failing checks behind it.
pub(crate) fn summarize(runs: &[CheckRun]) -> (CheckRollup, Vec<FailedCheck>) {
    let counted: Vec<&CheckRun> = latest(runs)
        .filter(|run| run.status != CheckStatus::Ignored)
        .collect();
    let failed: Vec<FailedCheck> = counted
        .iter()
        .filter(|run| run.status == CheckStatus::Failure)
        .map(|run| FailedCheck {
            workflow: run.workflow.clone(),
            event: run.event.clone(),
            name: run.name.clone(),
            details_url: run.details_url.clone(),
            job: run.details_url.as_deref().and_then(actions_job),
        })
        .collect();
    let rollup = if counted.is_empty() {
        CheckRollup::None
    } else if !failed.is_empty() {
        CheckRollup::Failing {
            failed: Some(u32::try_from(failed.len()).unwrap_or(u32::MAX)),
        }
    } else if counted.iter().any(|run| run.status == CheckStatus::Pending) {
        CheckRollup::Pending
    } else {
        CheckRollup::Passing
    };
    (rollup, failed)
}

/// The checks of `runs` to list, one entry per check as [`summarize`]
/// counts them: failing, then running, then passing, then skipped, each in
/// GitHub's order.
pub(crate) fn entries(runs: &[CheckRun]) -> Vec<CheckEntry> {
    let mut entries: Vec<CheckEntry> = latest(runs)
        .map(|run| CheckEntry {
            run: run.clone(),
            job: run.details_url.as_deref().and_then(actions_job),
            duration: duration(run),
        })
        .collect();
    entries.sort_by_key(|entry| match entry.run.status {
        CheckStatus::Failure => 0,
        CheckStatus::Pending => 1,
        CheckStatus::Success => 2,
        CheckStatus::Ignored => 3,
    });
    entries
}

/// The latest run of each check, in input order, so lists read in the order
/// GitHub gave them.
fn latest(runs: &[CheckRun]) -> impl Iterator<Item = &CheckRun> {
    let mut latest: HashMap<Key<'_>, &CheckRun> = HashMap::new();
    for run in runs {
        latest
            .entry(key(run))
            .and_modify(|kept| {
                if is_newer(run, kept) {
                    *kept = run;
                }
            })
            .or_insert(run);
    }
    runs.iter()
        .filter(move |run| std::ptr::eq(latest[&key(run)], *run))
}

/// From start to completion. A completion before the start (clock skew
/// between runners) or a malformed time gives none rather than a guess.
fn duration(run: &CheckRun) -> Option<Duration> {
    let started = humantime::parse_rfc3339(run.started_at.as_deref()?).ok()?;
    let completed = humantime::parse_rfc3339(run.completed_at.as_deref()?).ok()?;
    completed.duration_since(started).ok()
}

type Key<'a> = (CheckKind, Option<&'a str>, Option<&'a str>, &'a str);

fn key(run: &CheckRun) -> Key<'_> {
    (
        run.kind,
        run.workflow.as_deref(),
        run.event.as_deref(),
        run.name.as_str(),
    )
}

/// A queued run has not started yet, so it is newer than any run that has.
fn is_newer(run: &CheckRun, kept: &CheckRun) -> bool {
    match (&run.started_at, &kept.started_at) {
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (a, b) => a > b,
    }
}

/// The Actions job a check's details URL points at:
/// `https://<host>/<owner>/<repo>/actions/runs/<run>/job/<job>`.
pub(crate) fn actions_job(details_url: &str) -> Option<ActionsJob> {
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
mod tests;
