//! Workspace-owned recovery of a temporarily unusable Git executable.
use super::health::{GitReadResult, HealthEffect, ReadOutcome, ReadPermit};
use crate::lane::availability::LaneAvailability;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::{GitDirsState, RefreshSlot};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::system_info::redact_home;
use daruda_store::project::LaneRef;
use gpui::Context;

impl Workspace {
    /// Settle every Git read through the same outage and recovery policy.
    /// The callback also releases axis slots or directory probes when a
    /// result is stale; only accepted results may replace their cached data.
    pub(super) fn complete_git_read<R: GitReadResult>(
        &mut self,
        target: LaneRef,
        permit: ReadPermit,
        result: R,
        apply: impl FnOnce(&mut Self, R, bool, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) {
        if self.lane_scoped.contains_key(&target) {
            let outcome = result.outcome();
            let effect = self
                .git
                .health
                .finish(permit, outcome, cx.background_executor().now());
            if effect == HealthEffect::Report
                && let Some(message) = result.git_error_message()
            {
                self.report_git_environment_failure(target, message, cx);
            }
            let accepted =
                effect != HealthEffect::Stale && !matches!(outcome, ReadOutcome::Environment(_));
            apply(self, result, accepted, cx);
            if matches!(effect, HealthEffect::Recovered | HealthEffect::Cleared) {
                self.git_environment_recovered(target, cx);
            }
        } else {
            self.git
                .health
                .abandon(permit, cx.background_executor().now());
        }
        self.resume_manual_git_refreshes(cx);
        self.schedule_git_recovery(cx);
    }

    fn report_git_environment_failure(
        &mut self,
        target: LaneRef,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let mut report = ErrorReport::new(crate::surface::strings::error::git_status_failed())
            .severity(ErrorSeverity::Error)
            .message(message)
            .at(file!(), line!())
            .dedup("git.status");
        if let Some(lane) = self.lane_for(target) {
            report = report.with_context("path", redact_home(&lane.path));
        }
        self.report_error(report.build(), cx);
    }

    fn resume_manual_git_refreshes(&mut self, cx: &mut Context<Self>) {
        let pending: Vec<_> = self
            .lane_scoped
            .iter()
            .filter(|(_, state)| {
                state.git.manual_refresh_pending
                    && matches!(state.git.worktree_refresh, RefreshSlot::Idle)
            })
            .map(|(target, _)| *target)
            .collect();
        for target in pending {
            self.refresh_worktree_status(target, cx);
        }
    }

    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn git_failure_for_shot(
        &mut self,
        dismiss: bool,
        cx: &mut Context<Self>,
    ) {
        use super::health::ReadOrigin;
        let error = crate::lane::git::GitError::NotFound;
        let now = cx.background_executor().now();
        for _ in 0..3 {
            let Some(permit) = self.git.health.admit(ReadOrigin::Manual, now) else {
                return;
            };
            let outcome = ReadOutcome::Environment(
                crate::lane::git::GitEnvironmentFailure::ExecutableMissing,
            );
            if self.git.health.finish(permit, outcome, now) == HealthEffect::Report {
                self.report_git_environment_failure(self.active, error.to_string(), cx);
                if dismiss {
                    let id = self
                        .toast_layer
                        .read(cx)
                        .queue
                        .iter()
                        .last()
                        .map(|toast| toast.id);
                    if let Some(id) = id {
                        self.dismiss_error_toast(id, cx);
                    }
                }
            }
        }
    }

    pub(super) fn schedule_git_recovery(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.git.health.retry_at() else {
            self.git.recovery = None;
            return;
        };
        let eligible = self
            .lane_for(self.active)
            .is_some_and(|lane| lane.is_git() && lane.availability == LaneAvailability::Present);
        if !eligible {
            self.git.recovery = None;
            return;
        }
        if self
            .lane_scoped
            .get(&self.active)
            .is_some_and(|state| matches!(state.git.worktree_refresh, RefreshSlot::Running { .. }))
        {
            return;
        }
        if self.git.recovery.is_some() {
            return;
        }
        self.git.recovery = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(deadline.saturating_duration_since(cx.background_executor().now()))
                .await;
            // SILENT-OK: the recovery task belongs to the closed Workspace.
            let _ = this.update(cx, |ws, cx| {
                ws.git.recovery = None;
                ws.retry_git_environment(cx);
            });
        }));
    }

    fn retry_git_environment(&mut self, cx: &mut Context<Self>) {
        if self
            .lane_for(self.active)
            .is_some_and(|lane| lane.is_git() && lane.availability == LaneAvailability::Present)
        {
            self.refresh_worktree_status(self.active, cx);
        }
        self.schedule_git_recovery(cx);
    }

    pub(super) fn git_environment_recovered(
        &mut self,
        probe_target: daruda_store::project::LaneRef,
        cx: &mut Context<Self>,
    ) {
        self.git.recovery = None;
        for state in self.lane_scoped.values_mut() {
            if matches!(state.git.dirs, GitDirsState::Unavailable) {
                state.git.dirs = GitDirsState::Unknown;
            }
        }
        if probe_target != self.active {
            self.refresh_worktree_status(self.active, cx);
        }
        // Reads skipped during the outage include sibling badges after a write.
        let targets: Vec<_> = self
            .projects
            .lanes()
            .filter(|(_, _, lane)| lane.is_git())
            .map(|(target, _, _)| target)
            .collect();
        for target in targets {
            self.refresh_tracking(target, cx);
        }
        self.sync_git_watchers(cx);
    }
}

#[cfg(test)]
#[path = "recovery/tests.rs"]
mod tests;
