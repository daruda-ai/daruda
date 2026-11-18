//! What the detail can do to its pull request on GitHub: merge, close, mark
//! ready for review, and rerun failed checks. Merge and close ask first; each
//! runs `gh` on the background executor, one at a time per detail, and lands
//! only on the detail that asked (detail ids are never reused).

use daruda_github::{Gh, GhError, MergeMethod, PrDetail, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use gpui::{Context, Window};

use super::detail::PrDetailId;
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
    Reopening,
    EnablingAutoMerge,
    DisablingAutoMerge,
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
    /// GitHub took the last action, but nothing on the page shows it yet
    /// (a rerun starts later): said under the header until the next action
    /// or a reload.
    Done(String),
}

impl ActionState {
    pub(in crate::workspace) fn running(&self) -> Option<PrAction> {
        match self {
            Self::Running(action) => Some(*action),
            Self::Idle | Self::Failed(_) | Self::Done(_) => None,
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

impl PrAction {
    /// What this action failing on pull request `number` is called,
    /// wherever it is told.
    fn failure_title(self, number: u64, e: &GhError) -> String {
        match (e, self) {
            (GhError::HeadMoved, _) => error::github_pr_head_moved(number),
            (GhError::StackChanged, _) => error::github_pr_stack_changed(number),
            (_, Self::Merging) => error::github_pr_merge_failed(number),
            (_, Self::Closing) => error::github_pr_close_failed(number),
            (_, Self::Reopening) => error::github_pr_reopen_failed(number),
            (_, Self::EnablingAutoMerge) => error::github_pr_auto_merge_failed(number),
            (_, Self::DisablingAutoMerge) => error::github_pr_auto_merge_off_failed(number),
            (_, Self::MarkingReady) => error::github_pr_ready_failed(number),
            (_, Self::Rerunning) => error::github_pr_rerun_failed(number),
        }
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
        ErrorReport::new(action.failure_title(number, e))
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

/// One request to GitHub, with what it needs. Each knows when it applies
/// and how it is asked, so a new action is added here alone.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Merge {
        method: MergeMethod,
        head_sha: String,
    },
    Close,
    Reopen,
    /// Auto-merge on pull request `pr` (its node id), pinned to `head_sha`;
    /// with `stack`, the open pull requests of its stack below it too, as
    /// shown when asked (checked again before it is sent).
    AutoMergeOn {
        pr: String,
        method: MergeMethod,
        head_sha: String,
        stack: Option<Vec<u64>>,
    },
    AutoMergeOff {
        pr: String,
    },
    Ready,
    Rerun(Vec<u64>),
}

impl Request {
    fn action(&self) -> PrAction {
        match self {
            Self::Merge { .. } => PrAction::Merging,
            Self::Close => PrAction::Closing,
            Self::Reopen => PrAction::Reopening,
            Self::AutoMergeOn { .. } => PrAction::EnablingAutoMerge,
            Self::AutoMergeOff { .. } => PrAction::DisablingAutoMerge,
            Self::Ready => PrAction::MarkingReady,
            Self::Rerun(_) => PrAction::Rerunning,
        }
    }

    /// Whether `pr`, as the detail last read it, can take this request; the
    /// rules are `daruda_github`'s (see its `actions.rs`).
    fn applies(&self, pr: &PrDetail) -> bool {
        match self {
            Self::Merge { method, .. } => pr.can_merge(*method),
            Self::Close => pr.can_close_now(),
            Self::Reopen => pr.can_reopen_now(),
            Self::AutoMergeOn {
                pr: node, method, ..
            } => {
                pr.node_id.as_deref() == Some(node.as_str())
                    && pr.can_enable_auto_merge_with(*method)
            }
            Self::AutoMergeOff { pr: node } => {
                pr.node_id.as_deref() == Some(node.as_str()) && pr.can_disable_auto_merge_now()
            }
            Self::Ready => pr.is_draft(),
            // Only runs with a failed job: a rerun reruns those jobs.
            Self::Rerun(runs) => {
                let failed = pr.failed_runs();
                !runs.is_empty() && runs.iter().all(|run| failed.contains(run))
            }
        }
    }

    /// Ask GitHub. Blocking: runs off the UI thread.
    fn run(self, gh: &dyn Gh, repo: &RepoId, number: u64) -> Result<(), GhError> {
        match self {
            Self::Merge { method, head_sha } => gh.merge_pr(repo, number, method, &head_sha),
            Self::Close => gh.close_pr(repo, number),
            Self::Reopen => gh.reopen_pr(repo, number),
            Self::AutoMergeOn {
                pr,
                method,
                head_sha,
                stack,
            } => {
                // The range shown must still be the stack's: an edit to it
                // since is not merged on a guess.
                if let Some(shown) = &stack {
                    let now = match gh.pr_stack(repo, number)? {
                        daruda_github::StackRead::Stacked(read) => read.range_below(number),
                        daruda_github::StackRead::NotStacked
                        | daruda_github::StackRead::Unsupported => None,
                    };
                    if now.as_ref() != Some(shown) {
                        return Err(GhError::StackChanged);
                    }
                }
                gh.enable_auto_merge(repo, &pr, method, &head_sha, stack.is_some())
            }
            Self::AutoMergeOff { pr } => gh.disable_auto_merge(repo, &pr),
            Self::Ready => gh.set_ready(repo, number),
            // Every run is asked even after one fails; the first failure
            // is what is reported.
            Self::Rerun(runs) => runs
                .into_iter()
                .map(|run| gh.rerun_failed(repo, run))
                .fold(Ok(()), Result::and),
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
            .filter(|d| d.id() == id)?;
        let loaded = detail.idle_read()?;
        Some(Target {
            repo: detail.item().repo.clone(),
            number: loaded.summary.number,
            title: loaded.summary.title.clone(),
            base: loaded.base_branch.clone(),
            loaded: loaded.clone(),
        })
    }

    /// The merge menu's item: ask, then merge with `method` pinned to the
    /// head commit shown now. On a branch that requires a merge queue `gh pr
    /// merge` enqueues it (or turns on auto-merge) instead of merging now
    /// (gh 2.74.2 help); the reload after it shows which happened.
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
        let head_sha = target.loaded.head_sha.clone();
        let request = Request::Merge { method, head_sha };
        if !request.applies(&target.loaded) {
            return;
        }
        let ws = cx.weak_entity();
        open_confirm_dialog(
            s::merge_confirm_title(merge_action_label(method), target.number),
            s::merge_confirm_body(&target.title, &target.base),
            merge_action_label(method),
            ButtonVariant::Primary,
            move |_, _window, cx| {
                let request = request.clone();
                if let Some(ws) = ws.upgrade() {
                    ws.update(cx, |ws, cx| ws.run_pr_action(id, request, cx));
                }
            },
            window,
            cx,
        );
    }

    /// The merge menu's auto-merge item: ask, then turn auto-merge on with
    /// `method`, pinned to the head commit shown now. GitHub merges it (or
    /// queues it) later, once its requirements pass; never from a poll.
    pub(in crate::workspace) fn ask_pr_auto_merge(
        &mut self,
        id: PrDetailId,
        method: MergeMethod,
        stack: Option<Vec<u64>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        let Some(pr) = target.loaded.node_id.clone() else {
            return;
        };
        let request = Request::AutoMergeOn {
            pr,
            method,
            head_sha: target.loaded.head_sha.clone(),
            stack: stack.clone(),
        };
        if !request.applies(&target.loaded) {
            return;
        }
        let ws = cx.weak_entity();
        let body = match &stack {
            Some(range) => s::auto_merge_stack_confirm_body(
                &target.title,
                merge_action_label(method),
                range
                    .iter()
                    .map(|number| format!("#{number}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            None => {
                s::auto_merge_confirm_body(&target.title, merge_action_label(method), &target.base)
            }
        };
        open_confirm_dialog(
            s::auto_merge_confirm_title(target.number),
            body,
            s::auto_merge_confirm_ok(),
            ButtonVariant::Primary,
            move |_, _window, cx| {
                let request = request.clone();
                if let Some(ws) = ws.upgrade() {
                    ws.update(cx, |ws, cx| ws.run_pr_action(id, request, cx));
                }
            },
            window,
            cx,
        );
    }

    /// The header's Turn off auto-merge; nothing merges meanwhile, so it
    /// is not asked first.
    pub(in crate::workspace) fn disable_pr_auto_merge(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(pr) = self
            .pr_action_target(id)
            .and_then(|target| target.loaded.node_id)
        else {
            return;
        };
        self.run_pr_action(id, Request::AutoMergeOff { pr }, cx);
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

    /// The header's Reopen, on a pull request closed unmerged. Nothing is
    /// lost by it, so it is not asked first.
    pub(in crate::workspace) fn reopen_pr_detail(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.run_pr_action(id, Request::Reopen, cx);
    }

    /// The header's Ready for review, on a draft.
    pub(in crate::workspace) fn mark_pr_detail_ready(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        self.run_pr_action(id, Request::Ready, cx);
    }

    /// A failed check's own Rerun: the failed jobs of its Actions run.
    pub(in crate::workspace) fn rerun_pr_check_run(
        &mut self,
        id: PrDetailId,
        run: u64,
        cx: &mut Context<Self>,
    ) {
        self.run_pr_action(id, Request::Rerun(vec![run]), cx);
    }

    /// The Checks tab's Rerun failed checks: each failed run once.
    pub(in crate::workspace) fn rerun_pr_detail_checks(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.pr_action_target(id) {
            let runs = target.loaded.failed_runs();
            self.run_pr_action(id, Request::Rerun(runs), cx);
        }
    }

    fn run_pr_action(&mut self, id: PrDetailId, request: Request, cx: &mut Context<Self>) {
        let Some(target) = self.pr_action_target(id) else {
            return;
        };
        if !request.applies(&target.loaded) {
            return;
        }
        let action = request.action();
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.begin_action(action);
        }
        let (repo, number) = (target.repo, target.number);
        let gh = GhHost::global(cx);
        let bg_repo = repo.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { request.run(&*gh, &bg_repo, number) })
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

    /// What was shown is out of date and nothing was sent: a warning whose
    /// title already says it all, without `gh`'s words after. Returns the
    /// line said under the header.
    #[track_caller]
    fn report_out_of_date(
        &mut self,
        title: String,
        dedup: &'static str,
        cx: &mut Context<Self>,
    ) -> String {
        let at = std::panic::Location::caller();
        let report = ErrorReport::new(title.clone())
            .severity(ErrorSeverity::Warning)
            .at(at.file(), at.line())
            .dedup(dedup)
            .build();
        self.report_error(report, cx);
        title
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
        let asker_waiting = self
            .pr_detail_mut(id)
            .is_some_and(|detail| detail.finish_action(action));
        let failure = match &result {
            Ok(()) => {
                PrPoller::nudge(repo, cx);
                None
            }
            Err(e @ GhError::HeadMoved) => Some(self.report_out_of_date(
                action.failure_title(number, e),
                "github.pr_action.head_moved",
                cx,
            )),
            Err(e @ GhError::StackChanged) => Some(self.report_out_of_date(
                action.failure_title(number, e),
                "github.pr_action.stack_changed",
                cx,
            )),
            Err(e) => {
                let title = action.failure_title(number, e);
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
        // After the reload, which clears an earlier line.
        if asker_waiting && let Some(detail) = self.pr_detail_mut(id) {
            match failure {
                Some(line) => detail.fail_action(line),
                // A rerun shows nowhere until GitHub starts the jobs.
                None if action == PrAction::Rerunning => detail.done_action(s::rerun_requested()),
                None => {}
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: daruda_github::PrState, mergeability: daruda_github::Mergeability) -> PrDetail {
        use daruda_github::{CheckRollup, PrSummary};
        PrDetail {
            summary: PrSummary {
                number: 7,
                url: String::new(),
                title: String::new(),
                state,
                head_branch: "feat".into(),
                cross_repo: false,
                checks: CheckRollup::None,
                failed: Vec::new(),
                review: None,
                updated_at: String::new(),
            },
            body: String::new(),
            head_sha: "abc".into(),
            base_branch: "main".into(),
            author: None,
            mergeability,
            methods: vec![MergeMethod::Squash],
            default_method: None,
            reviewers: Vec::new(),
            checks: Vec::new(),
            checks_truncated: false,
            open_threads: 0,
            open_threads_more: false,
            can_update: false,
            can_close: true,
            can_reopen: true,
            auto_merge: None,
            queue: None,
            can_enable_auto_merge: true,
            can_disable_auto_merge: true,
            node_id: Some("PR_7".into()),
            changed_files: None,
        }
    }

    #[test]
    fn each_request_applies_only_where_github_would_take_it() {
        use daruda_github::{Mergeability as M, PrState as S};
        let merge = |method| Request::Merge {
            method,
            head_sha: "abc".into(),
        };
        let open = pr(S::Open, M::Ready);
        let draft = pr(S::Draft, M::Unknown);
        let conflicting = pr(S::Open, M::Conflicting);
        let waiting = pr(
            S::Open,
            M::Blocked(daruda_github::BlockReason::Requirements),
        );
        let closed = pr(S::Closed, M::Unknown);
        let failing = PrDetail {
            checks: vec![daruda_github::CheckEntry {
                run: daruda_github::CheckRun {
                    kind: daruda_github::CheckKind::Run,
                    workflow: None,
                    event: None,
                    name: "test".into(),
                    status: daruda_github::CheckStatus::Failure,
                    details_url: None,
                    started_at: None,
                    completed_at: None,
                },
                job: Some(daruda_github::ActionsJob { run: 1, job: 10 }),
                duration: None,
            }],
            ..pr(S::Open, M::Ready)
        };
        let merged = pr(S::Merged, M::Unknown);
        let not_mine = PrDetail {
            can_close: false,
            can_reopen: false,
            ..pr(S::Closed, M::Unknown)
        };
        let open_not_mine = PrDetail {
            can_close: false,
            ..pr(S::Open, M::Ready)
        };
        // (label, request, pull request, applies)
        let cases = [
            ("merge an open PR", merge(MergeMethod::Squash), &open, true),
            (
                "a method the repository refuses",
                merge(MergeMethod::Rebase),
                &open,
                false,
            ),
            ("merge a draft", merge(MergeMethod::Squash), &draft, false),
            (
                "merge a conflict",
                merge(MergeMethod::Squash),
                &conflicting,
                false,
            ),
            ("close a draft", Request::Close, &draft, true),
            ("close a closed PR", Request::Close, &closed, false),
            (
                "auto-merge a PR waiting on requirements",
                Request::AutoMergeOn {
                    pr: "PR_7".into(),
                    method: MergeMethod::Squash,
                    head_sha: "abc".into(),
                    stack: None,
                },
                &waiting,
                true,
            ),
            (
                "auto-merge another PR's node",
                Request::AutoMergeOn {
                    pr: "PR_8".into(),
                    method: MergeMethod::Squash,
                    head_sha: "abc".into(),
                    stack: None,
                },
                &waiting,
                false,
            ),
            (
                "auto-merge a draft",
                Request::AutoMergeOn {
                    pr: "PR_7".into(),
                    method: MergeMethod::Squash,
                    head_sha: "abc".into(),
                    stack: None,
                },
                &draft,
                false,
            ),
            (
                "auto-merge a PR that can merge now",
                Request::AutoMergeOn {
                    pr: "PR_7".into(),
                    method: MergeMethod::Squash,
                    head_sha: "abc".into(),
                    stack: None,
                },
                &open,
                false,
            ),
            (
                "turn off auto-merge that is off",
                Request::AutoMergeOff { pr: "PR_7".into() },
                &open,
                false,
            ),
            ("reopen a closed PR", Request::Reopen, &closed, true),
            ("reopen an open PR", Request::Reopen, &open, false),
            ("reopen a merged PR", Request::Reopen, &merged, false),
            (
                "reopen without permission",
                Request::Reopen,
                &not_mine,
                false,
            ),
            (
                "close without permission",
                Request::Close,
                &open_not_mine,
                false,
            ),
            ("mark a draft ready", Request::Ready, &draft, true),
            ("mark an open PR ready", Request::Ready, &open, false),
            ("rerun failed runs", Request::Rerun(vec![1]), &failing, true),
            (
                "rerun a run that did not fail",
                Request::Rerun(vec![1, 2]),
                &failing,
                false,
            ),
            (
                "rerun on a PR with no failure",
                Request::Rerun(vec![1]),
                &open,
                false,
            ),
            ("rerun nothing", Request::Rerun(Vec::new()), &open, false),
        ];
        for (label, request, pr, applies) in cases {
            assert_eq!(request.applies(pr), applies, "{label}");
        }
    }

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
