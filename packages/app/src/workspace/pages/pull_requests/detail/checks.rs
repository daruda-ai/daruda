//! The Checks tab's state and reads: which checks are listed, and the logs
//! of failed Actions jobs, opened on request.

use daruda_github::{ActionsJob, GhError};
use gpui::Context;

use super::{Load, PrDetailId, PrDetailState, PrTab, ReadSlot, ReadTicket, split};
use crate::gh_host::GhHost;
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// Which checks the tab lists. Chosen by how the tab was reached, and back
/// to all of them whenever a tab is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::workspace) enum ChecksShown {
    #[default]
    All,
    /// The failing ones alone, as "inspect failures" asks.
    Failures,
}

/// A failed Actions job's log, keyed as `(run, job)`.
pub(super) type JobKey = (u64, u64);

pub(super) fn job_key(job: ActionsJob) -> JobKey {
    (job.run, job.job)
}

impl PrDetailState {
    /// Close `job`'s log if open (`None`), else open it with a new read.
    fn toggle_log(&mut self, job: ActionsJob) -> Option<ReadTicket> {
        if self.logs.remove(&job_key(job)).is_some() {
            return None;
        }
        let ticket = self.tickets.issue();
        self.logs.insert(job_key(job), ReadSlot::reading(ticket));
        Some(ticket)
    }

    pub(in crate::workspace) fn log(&self, job: ActionsJob) -> Option<&Load<String>> {
        self.logs.get(&job_key(job)).map(ReadSlot::load)
    }

    /// The read `job`'s open log waits on, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn log_ticket(&self, job: ActionsJob) -> Option<ReadTicket> {
        self.logs.get(&job_key(job))?.waiting()
    }
}

impl Workspace {
    /// Open a sole failing Actions log directly. Multiple failures stay listed together.
    pub(in crate::workspace) fn inspect_failed_pr_checks(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.select_pr_detail_tab(id, PrTab::Checks, cx);
        let job = self.pr_detail_mut(id).and_then(|detail| {
            detail.checks_shown = ChecksShown::Failures;
            detail.scroll.set_offset(Default::default());
            let pr = detail.pr.confirmed()?;
            let mut failed = pr
                .checks
                .iter()
                .filter(|entry| entry.run.status == daruda_github::CheckStatus::Failure);
            let job = failed.next()?.job?;
            (failed.next().is_none() && !pr.checks_truncated && detail.log(job).is_none())
                .then_some(job)
        });
        if let Some(job) = job {
            self.toggle_pr_check_log(id, job, cx);
        }
    }

    /// Toggle a failed Actions job’s log, reading it on first open.
    pub(in crate::workspace) fn toggle_pr_check_log(
        &mut self,
        id: PrDetailId,
        job: ActionsJob,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(ticket) = detail.toggle_log(job) else {
            cx.notify();
            return;
        };
        let repo = detail.item.repo.clone();
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || {
                gh.failed_log(&repo, job)
                    .map(|raw| daruda_github::prompt::clean_log(&raw))
            },
            move |ws, result, cx| ws.land_pr_check_log(id, job, ticket, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Only the read the open log waits for lands; an earlier read of the
    /// same job, from before a close or a refresh, is dropped.
    pub(in crate::workspace) fn land_pr_check_log(
        &mut self,
        id: PrDetailId,
        job: ActionsJob,
        ticket: ReadTicket,
        result: Result<String, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.logs.get_mut(&job_key(job)))
        else {
            return;
        };
        let (answer, error) = split(result);
        if !slot.land(ticket, answer) {
            return;
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_log.read", cx);
        }
        cx.notify();
    }
}
