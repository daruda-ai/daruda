//! The Checks tab's rows and their logs.

use super::*;

/// A failed Actions check's log, which it can open under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum LogSlot {
    /// Not a failed Actions job: nothing to open.
    None,
    Closed(ActionsJob),
    Open(ActionsJob, Load<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct CheckRowView {
    pub status: CheckStatus,
    pub name: String,
    /// The check's page on GitHub, when it has one that is a web page.
    pub url: Option<String>,
    /// A failed Actions job's run, to rerun its failed jobs.
    pub rerun: Option<(u64, ActionSlot)>,
    /// `workflow · event`, for an Actions check.
    pub context: Option<String>,
    pub duration: Option<String>,
    pub log: LogSlot,
}

/// `2 failing · 5 passed · 1 skipped`, leaving out what there is none of.
pub(super) fn checks_summary(loaded: &PrDetail) -> String {
    if loaded.checks_truncated {
        let verdict =
            crate::workspace::left_dock::git_changes::pr_strip::checks_label(loaded.summary.checks)
                .unwrap_or_else(s::checks_none);
        return s::checks_truncated(verdict);
    }
    let checks = &loaded.checks;
    let count = |status| checks.iter().filter(|e| e.run.status == status).count();
    let mut parts = Vec::new();
    let failing = count(CheckStatus::Failure);
    if failing > 0 {
        parts.push(s::checks_failing(failing));
    }
    let running = count(CheckStatus::Pending);
    if running > 0 {
        parts.push(s::checks_running(running));
    }
    let passed = count(CheckStatus::Success);
    if passed > 0 {
        parts.push(s::checks_passed(passed));
    }
    let skipped = count(CheckStatus::Ignored);
    if skipped > 0 {
        parts.push(s::checks_skipped(skipped));
    }
    if parts.is_empty() {
        s::checks_none()
    } else {
        parts.join(" · ")
    }
}

pub(super) fn check_row(entry: &CheckEntry, state: &PrDetailState) -> CheckRowView {
    let rerun = match entry.job {
        Some(job) if entry.run.status == CheckStatus::Failure => Some((
            job.run,
            ActionSlot::of(&state.action, PrAction::Rerunning, None),
        )),
        _ => None,
    };
    let run = &entry.run;
    let context: Vec<&str> = [run.workflow.as_deref(), run.event.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let log = match entry.job {
        Some(job) if run.status == CheckStatus::Failure => match state.log(job) {
            Some(load) => LogSlot::Open(job, load.clone()),
            None => LogSlot::Closed(job),
        },
        _ => LogSlot::None,
    };
    CheckRowView {
        status: run.status,
        name: run.name.clone(),
        url: run
            .details_url
            .as_deref()
            .filter(|url| daruda_github::model::is_web_url(url))
            .map(str::to_owned),
        rerun,
        context: (!context.is_empty()).then(|| context.join(" · ")),
        duration: entry.duration.map(notification::format_duration_compact),
        log,
    }
}
