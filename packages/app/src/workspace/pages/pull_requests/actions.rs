//! What the detail can do to its pull request on GitHub: merge, close, mark
//! ready for review, and rerun failed checks. Merge and close ask first; each
//! runs `gh` on the background executor, one at a time per detail, and lands
//! only on the detail that asked (detail ids are never reused).

use std::collections::BTreeSet;

use daruda_github::{
    CheckEntry, CheckStatus, GhError, MergeMethod, Mergeability, PrDetail, PrState, RepoId,
};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{Context, Window};

use super::detail::{Load, PrDetailId};
use super::detail_view::merge_action_label;
use crate::gh_host::GhHost;
use crate::github::PrPoller;
use crate::surface::strings::{error, pull_requests as s};
use crate::ui::dialog::{ButtonVariant, confirm_destructive, open_confirm_dialog};
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// What the detail is doing to its pull request; every action button is off
/// while one runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::workspace) enum PrAction {
    #[default]
    Idle,
    Merging,
    Closing,
    MarkingReady,
    Rerunning,
}

/// The merge methods the menu offers: only those the repository allows,
/// GitHub's preselected one for the viewer first, since it labels the
/// button. Without one (or one no longer allowed) a fixed order stands in:
/// Squash, then Merge, then Rebase.
pub(in crate::workspace) fn merge_menu(
    allowed: &[MergeMethod],
    default: Option<MergeMethod>,
) -> Vec<MergeMethod> {
    let fallback = [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase];
    let mut menu: Vec<MergeMethod> = default
        .into_iter()
        .chain(fallback.into_iter().filter(|m| Some(*m) != default))
        .filter(|method| allowed.contains(method))
        .collect();
    menu.dedup();
    menu
}

/// Each Actions run with a failed job, once: `gh run rerun --failed` reruns
/// every failed job of a run, so two failed jobs of one run are one rerun.
pub(in crate::workspace) fn failed_runs(checks: &[CheckEntry]) -> Vec<u64> {
    let runs: BTreeSet<u64> = checks
        .iter()
        .filter(|entry| entry.run.status == CheckStatus::Failure)
        .filter_map(|entry| entry.job.map(|job| job.run))
        .collect();
    runs.into_iter().collect()
}

/// One request to GitHub, with what it needs.
enum Request {
    Merge {
        method: MergeMethod,
        head_sha: String,
    },
    Close,
    Ready,
    Rerun(Vec<u64>),
}

impl Request {
    fn action(&self) -> PrAction {
        match self {
            Self::Merge { .. } => PrAction::Merging,
            Self::Close => PrAction::Closing,
            Self::Ready => PrAction::MarkingReady,
            Self::Rerun(_) => PrAction::Rerunning,
        }
    }
}

/// The pull request an action is about, as the detail last read it.
struct Target {
    repo: RepoId,
    number: u64,
    title: String,
    base: String,
    loaded: PrDetail,
}

impl Workspace {
    /// The shown detail's pull request when no action is running on it.
    fn pr_action_target(&self, id: PrDetailId) -> Option<Target> {
        let detail = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.id == id && d.action == PrAction::Idle)?;
        let Load::Ready(loaded) = &detail.load else {
            return None;
        };
        Some(Target {
            repo: detail.item.repo.clone(),
            number: loaded.summary.number,
            title: loaded.summary.title.clone(),
            base: loaded.base_branch.clone(),
            loaded: loaded.clone(),
        })
    }

    /// The merge menu's item: ask, then merge with `method` pinned to the
    /// head commit shown now.
    pub(in crate::workspace) fn ask_pr_merge(
        &mut self,
        id: PrDetailId,
        method: MergeMethod,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        let allowed = target.loaded.mergeability == Mergeability::Ready
            && target.loaded.methods.contains(&method);
        if !allowed {
            return;
        }
        let head_sha = target.loaded.head_sha.clone();
        let ws = cx.weak_entity();
        open_confirm_dialog(
            s::merge_confirm_title(merge_action_label(method), target.number),
            s::merge_confirm_body(&target.title, &target.base),
            merge_action_label(method),
            ButtonVariant::Primary,
            move |_, _window, cx| {
                let head_sha = head_sha.clone();
                if let Some(ws) = ws.upgrade() {
                    ws.update(cx, |ws, cx| ws.merge_pr_detail(id, method, head_sha, cx));
                }
            },
            window,
            cx,
        );
    }

    /// Merge, after the dialog's OK. On a branch that requires a merge queue
    /// `gh pr merge` enqueues it (or turns on auto-merge) instead of merging
    /// now (gh 2.74.2 help); the reload after it shows which happened.
    fn merge_pr_detail(
        &mut self,
        id: PrDetailId,
        method: MergeMethod,
        head_sha: String,
        cx: &mut Context<Self>,
    ) {
        self.run_pr_action(id, Request::Merge { method, head_sha }, cx);
    }

    /// The header's Close: ask, then close without merging.
    pub(in crate::workspace) fn ask_pr_close(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        confirm_destructive(
            cx.weak_entity(),
            s::close_confirm_title(target.number),
            s::close_confirm_body(&target.title),
            s::close_confirm_ok(),
            move |ws, _window, cx| ws.run_pr_action(id, Request::Close, cx),
            window,
            cx,
        );
    }

    /// The header's Ready for review, on a draft.
    pub(in crate::workspace) fn mark_pr_detail_ready(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.run_pr_action(id, Request::Ready, cx);
    }

    /// The Checks tab's Rerun failed checks: each failed run once.
    pub(in crate::workspace) fn rerun_pr_detail_checks(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        let runs = failed_runs(&target.loaded.checks);
        if !runs.is_empty() {
            self.run_pr_action(id, Request::Rerun(runs), cx);
        }
    }

    fn run_pr_action(&mut self, id: PrDetailId, request: Request, cx: &mut Context<Self>) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        let applies = match &request {
            Request::Merge { method, .. } => {
                target.loaded.mergeability == Mergeability::Ready
                    && target.loaded.methods.contains(method)
            }
            Request::Close => true,
            Request::Ready => target.loaded.summary.state == PrState::Draft,
            Request::Rerun(runs) => !runs.is_empty(),
        };
        if !applies {
            return;
        }
        let action = request.action();
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.action = action;
        }
        let (repo, number) = (target.repo, target.number);
        let gh = GhHost::global(cx);
        let bg_repo = repo.clone();
        spawn_bg_work_and_mutate(
            cx,
            move || match request {
                Request::Merge { method, head_sha } => {
                    gh.merge_pr(&bg_repo, number, method, &head_sha)
                }
                Request::Close => gh.close_pr(&bg_repo, number),
                Request::Ready => gh.set_ready(&bg_repo, number),
                // Every run is asked even after one fails; the first failure
                // is what is reported.
                Request::Rerun(runs) => runs
                    .into_iter()
                    .map(|run| gh.rerun_failed(&bg_repo, run))
                    .fold(Ok(()), Result::and),
            },
            move |ws, result, cx| ws.land_pr_action(id, action, repo, number, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// GitHub changed (or may have) whichever detail is shown now, so the
    /// list and the lane badges are asked again either way; only the detail
    /// that asked is reloaded or freed. A failure is reported even if that
    /// detail is gone: the person asked for the action and it did not happen.
    fn land_pr_action(
        &mut self,
        id: PrDetailId,
        action: PrAction,
        repo: RepoId,
        number: u64,
        result: Result<(), GhError>,
        cx: &mut Context<Self>,
    ) {
        let shown = match self.pr_detail_mut(id) {
            Some(detail) if detail.action == action => {
                detail.action = PrAction::Idle;
                true
            }
            _ => false,
        };
        match &result {
            Ok(()) => PrPoller::nudge(repo, cx),
            Err(GhError::HeadMoved) => {
                let report = ErrorReport::new(error::github_pr_head_moved(number))
                    .severity(ErrorSeverity::Warning)
                    .at(file!(), line!())
                    .dedup("github.pr_action.head_moved")
                    .build();
                self.report_error(report, cx);
            }
            Err(e) => {
                let title = match action {
                    PrAction::Merging => error::github_pr_merge_failed(number),
                    PrAction::Closing => error::github_pr_close_failed(number),
                    PrAction::MarkingReady => error::github_pr_ready_failed(number),
                    PrAction::Rerunning | PrAction::Idle => error::github_pr_rerun_failed(number),
                };
                let report = ErrorReport::new(title)
                    .severity(ErrorSeverity::Error)
                    .from_error(e)
                    .at(file!(), line!())
                    .dedup("github.pr_action")
                    .build();
                self.report_error(report, cx);
            }
        }
        // A failure other than a moved head leaves the pull request as shown.
        let reload = result.is_ok() || matches!(result, Err(GhError::HeadMoved));
        if shown && reload {
            self.refresh_pr_detail(id, cx);
        } else if result.is_ok() {
            self.refresh_pr_list(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use daruda_github::{ActionsJob, CheckKind, CheckRun};

    use super::*;

    #[test]
    fn the_menu_offers_only_allowed_methods_default_first() {
        use MergeMethod::{Merge, Rebase, Squash};
        // (label, allowed as GitHub lists them, GitHub's default, menu)
        let cases: [(
            &str,
            Vec<MergeMethod>,
            Option<MergeMethod>,
            Vec<MergeMethod>,
        ); 8] = [
            (
                "the default leads",
                vec![Merge, Squash, Rebase],
                Some(Rebase),
                vec![Rebase, Squash, Merge],
            ),
            (
                "merge as default",
                vec![Merge, Squash, Rebase],
                Some(Merge),
                vec![Merge, Squash, Rebase],
            ),
            (
                "a default no longer allowed falls back",
                vec![Merge, Rebase],
                Some(Squash),
                vec![Merge, Rebase],
            ),
            (
                "no default: the fixed order",
                vec![Merge, Squash, Rebase],
                None,
                vec![Squash, Merge, Rebase],
            ),
            ("no squash", vec![Rebase, Merge], None, vec![Merge, Rebase]),
            ("rebase only", vec![Rebase], Some(Rebase), vec![Rebase]),
            (
                "squash and rebase",
                vec![Rebase, Squash],
                None,
                vec![Squash, Rebase],
            ),
            ("none", vec![], Some(Merge), vec![]),
        ];
        for (label, allowed, default, menu) in cases {
            assert_eq!(merge_menu(&allowed, default), menu, "{label}");
        }
    }

    fn entry(status: CheckStatus, job: Option<ActionsJob>) -> CheckEntry {
        CheckEntry {
            run: CheckRun {
                kind: CheckKind::Run,
                workflow: None,
                event: None,
                name: "c".into(),
                status,
                details_url: None,
                started_at: None,
                completed_at: None,
            },
            job,
            duration: None,
        }
    }

    #[test]
    fn each_failed_run_is_rerun_once() {
        let job = |run, job| Some(ActionsJob { run, job });
        let checks = [
            entry(CheckStatus::Failure, job(9, 1)),
            entry(CheckStatus::Failure, job(9, 2)),
            entry(CheckStatus::Failure, job(4, 3)),
            entry(CheckStatus::Failure, None),
            entry(CheckStatus::Success, job(5, 4)),
            entry(CheckStatus::Pending, job(6, 5)),
        ];
        assert_eq!(failed_runs(&checks), [4, 9]);
        assert!(failed_runs(&checks[3..]).is_empty());
    }
}
