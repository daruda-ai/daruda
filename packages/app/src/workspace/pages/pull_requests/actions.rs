//! What the detail can do to its pull request on GitHub: merge, close, mark
//! ready for review, and rerun failed checks. Merge and close ask first; each
//! runs `gh` on the background executor, one at a time per detail, and lands
//! only on the detail that asked (detail ids are never reused).

use daruda_github::{GhError, MergeMethod, PrDetail, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use gpui::{Context, Window};

use super::detail::{Load, PrDetailId};
use crate::gh_host::GhHost;
use crate::github::PrPoller;
use crate::surface::strings::{error, pull_requests as s};
use crate::ui::dialog::{ButtonVariant, confirm_destructive, open_confirm_dialog};
use crate::workspace::Workspace;

/// An action the detail runs on its pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrAction {
    Merging,
    Closing,
    MarkingReady,
    Rerunning,
}

/// Where the detail's actions stand; every action button is off while one
/// runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::workspace) enum ActionState {
    #[default]
    Idle,
    Running(PrAction),
    /// The last action failed: what it was and what `gh` said, shown under
    /// the header until the next action or a reload. Actions can run again.
    Failed(String),
}

impl ActionState {
    pub(in crate::workspace) fn running(&self) -> Option<PrAction> {
        match self {
            Self::Running(action) => Some(*action),
            Self::Idle | Self::Failed(_) => None,
        }
    }
}

/// What merging with `method` is called on its button, menu item and
/// confirmation.
pub(in crate::workspace) fn merge_action_label(method: MergeMethod) -> String {
    match method {
        MergeMethod::Squash => s::merge_squash(),
        MergeMethod::Merge => s::merge_commit(),
        MergeMethod::Rebase => s::merge_rebase(),
    }
}

/// What a failed `action` on pull request `number` is called, wherever it
/// is told.
fn failure_title(action: PrAction, number: u64, e: &GhError) -> String {
    match (e, action) {
        (GhError::HeadMoved, _) => error::github_pr_head_moved(number),
        (_, PrAction::Merging) => error::github_pr_merge_failed(number),
        (_, PrAction::Closing) => error::github_pr_close_failed(number),
        (_, PrAction::MarkingReady) => error::github_pr_ready_failed(number),
        (_, PrAction::Rerunning) => error::github_pr_rerun_failed(number),
    }
}

/// What the log keeps of an action whose window closed while `gh` ran:
/// with no workspace to show it, the log is the only place the person can
/// learn it failed. `None` when it did not fail.
fn unshown_failure(
    action: PrAction,
    repo: &RepoId,
    number: u64,
    result: &Result<(), GhError>,
) -> Option<ErrorReport> {
    let e = result.as_ref().err()?;
    Some(
        ErrorReport::new(failure_title(action, number, e))
            .severity(ErrorSeverity::Error)
            .from_error(e)
            .with_context(
                "repo",
                format!("{}/{}/{}", repo.host(), repo.owner(), repo.name()),
            )
            .at(file!(), line!())
            .dedup("github.pr_action.closed")
            .build(),
    )
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
            .filter(|d| d.id == id && d.action.running().is_none())?;
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
        if !target.loaded.can_merge(method) {
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
        let runs = target.loaded.failed_runs();
        if !runs.is_empty() {
            self.run_pr_action(id, Request::Rerun(runs), cx);
        }
    }

    fn run_pr_action(&mut self, id: PrDetailId, request: Request, cx: &mut Context<Self>) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        let applies = match &request {
            Request::Merge { method, .. } => target.loaded.can_merge(*method),
            Request::Close => true,
            Request::Ready => target.loaded.is_draft(),
            Request::Rerun(runs) => !runs.is_empty(),
        };
        if !applies {
            return;
        }
        let action = request.action();
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.action = ActionState::Running(action);
        }
        let (repo, number) = (target.repo, target.number);
        let gh = GhHost::global(cx);
        let bg_repo = repo.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match request {
                        Request::Merge { method, head_sha } => {
                            gh.merge_pr(&bg_repo, number, method, &head_sha)
                        }
                        Request::Close => gh.close_pr(&bg_repo, number),
                        Request::Ready => gh.set_ready(&bg_repo, number),
                        // Every run is asked even after one fails; the first
                        // failure is what is reported.
                        Request::Rerun(runs) => runs
                            .into_iter()
                            .map(|run| gh.rerun_failed(&bg_repo, run))
                            .fold(Ok(()), Result::and),
                    }
                })
                .await;
            // Built before the result moves into the update, which may not run.
            let unshown = unshown_failure(action, &repo, number, &result);
            let landed = this.update(cx, |ws, cx| {
                ws.land_pr_action(id, action, repo, number, result, cx)
            });
            if let (Err(_), Some(report)) = (landed, unshown) {
                LogWriter::log(report);
            }
        })
        .detach();
        cx.notify();
    }

    /// GitHub changed (or may have) whichever detail is shown now, so the
    /// list and the lane badges are asked again either way; only the detail
    /// that asked is reloaded or freed. A failure is reported even if that
    /// detail is gone: the person asked for the action and it may not have
    /// happened. The detail that asked keeps what failed, under its header,
    /// and is reloaded either way: a failure does not prove GitHub did
    /// nothing (a merge can land after `gh` timed out).
    fn land_pr_action(
        &mut self,
        id: PrDetailId,
        action: PrAction,
        repo: RepoId,
        number: u64,
        result: Result<(), GhError>,
        cx: &mut Context<Self>,
    ) {
        // The detail that asked is still there, waiting on this action.
        let asker_waiting = match self.pr_detail_mut(id) {
            Some(detail) if detail.action == ActionState::Running(action) => {
                detail.action = ActionState::Idle;
                true
            }
            _ => false,
        };
        let failure = match &result {
            Ok(()) => {
                PrPoller::nudge(repo, cx);
                None
            }
            Err(e @ GhError::HeadMoved) => {
                let title = failure_title(action, number, e);
                let report = ErrorReport::new(title.clone())
                    .severity(ErrorSeverity::Warning)
                    .at(file!(), line!())
                    .dedup("github.pr_action.head_moved")
                    .build();
                self.report_error(report, cx);
                Some(title)
            }
            Err(e) => {
                let title = failure_title(action, number, e);
                let line = s::action_failed(&title, e);
                let report = ErrorReport::new(title)
                    .severity(ErrorSeverity::Error)
                    .from_error(e)
                    .at(file!(), line!())
                    .dedup("github.pr_action")
                    .build();
                self.report_error(report, cx);
                Some(line)
            }
        };
        if asker_waiting {
            self.refresh_pr_detail(id, cx);
        } else if result.is_ok() {
            self.refresh_pr_list(cx);
        }
        // After the reload, which clears an earlier failure.
        if let (true, Some(line)) = (asker_waiting, failure)
            && let Some(detail) = self.pr_detail_mut(id)
        {
            detail.action = ActionState::Failed(line);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_no_window_can_show_goes_to_the_log() {
        let repo = RepoId::new("github.com", "o", "r");
        assert!(
            unshown_failure(PrAction::Merging, &repo, 7, &Ok(())).is_none(),
            "a success has nothing to tell"
        );
        let timeout = Err(GhError::Timeout(std::time::Duration::from_secs(30)));
        let report =
            unshown_failure(PrAction::Merging, &repo, 7, &timeout).expect("a failure is logged");
        assert_eq!(report.title, error::github_pr_merge_failed(7));
        assert_eq!(report.dedup_key.as_deref(), Some("github.pr_action.closed"));
        let moved = unshown_failure(PrAction::Merging, &repo, 7, &Err(GhError::HeadMoved))
            .expect("a moved head is logged");
        assert_eq!(moved.title, error::github_pr_head_moved(7));
    }
}
