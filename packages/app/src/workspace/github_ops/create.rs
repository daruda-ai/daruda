//! Opening a pull request for the active lane: push what GitHub has not seen
//! yet, then `gh pr create`, under the repository lock so Push and Fetch wait.

use daruda_github::remote_parse::parse_remote;
use daruda_github::{GhError, NewPr};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::system_info::redact_home;
use gpui::{AnyWindowHandle, Context, WeakEntity, Window};

use super::create_modal::CreatePrModal;
use crate::gh_host::GhHost;
use crate::git_host::GitHost;
use crate::github::PrPoller;
use crate::lane::git::GitError;
use crate::ui::WindowExt as _;
use crate::workspace::Workspace;
use crate::workspace::left_dock::git_ops::lock::GitLock;

/// What the modal hands over; the lane supplies the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreatePrRequest {
    pub title: String,
    pub body: String,
    /// `None` lets GitHub pick the repository's default branch.
    pub base: Option<String>,
    pub draft: bool,
}

#[derive(Debug)]
pub(crate) enum CreatePrError {
    Push(GitError),
    Gh(GhError),
}

impl std::fmt::Display for CreatePrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Push(e) => write!(f, "push before creating the pull request: {e}"),
            Self::Gh(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CreatePrError {}

/// What the push step must do first, from the lane's push target and how
/// far it is ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PushFirst {
    /// The branch was never pushed: push it and set its upstream.
    Publish,
    /// Pushed before, with commits since.
    Update,
    Nothing,
}

fn push_first(published: bool, ahead: u32) -> PushFirst {
    match (published, ahead) {
        (false, _) => PushFirst::Publish,
        (true, 0) => PushFirst::Nothing,
        (true, _) => PushFirst::Update,
    }
}

impl Workspace {
    /// The base a new PR targets by default: the lane's own base without its
    /// remote prefix, else the repository's default branch, else nothing.
    fn default_pr_base(&self, cx: &gpui::App) -> Option<String> {
        let state = self.lane_scoped.get(&self.active)?;
        let push = state.git.push_target.as_ref()?;
        let lane_base = self
            .lane_for(self.active)
            .and_then(|lane| lane.base_ref.clone());
        let stripped = lane_base.map(|base| {
            base.strip_prefix(&format!("{}/", push.remote))
                .map(str::to_owned)
                .unwrap_or(base)
        });
        stripped.or_else(|| {
            let repo = parse_remote(&push.url)?;
            PrPoller::default_branch(&repo, cx)
        })
    }

    pub(in crate::workspace) fn open_create_pr_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.git_lane_path(self.active) else {
            return;
        };
        let base = self.default_pr_base(cx);
        let workspace = cx.weak_entity();
        let repo = GitHost::global(cx).open(&path);
        crate::workspace::dialog_helpers::open_form_modal(
            crate::surface::strings::git::pr_create_title(),
            None,
            move |window, cx| CreatePrModal::new(workspace, repo, base, window, cx),
            window,
            cx,
        );
    }

    /// Push if needed, then open the PR. `modal` hears how it went; on
    /// success the dialog in `window` closes and the poller asks at once.
    pub(in crate::workspace) fn create_pr(
        &mut self,
        request: CreatePrRequest,
        modal: WeakEntity<CreatePrModal>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let target = self.active;
        let state = self.lane_scoped.get(&target);
        let Some(push) = state.and_then(|s| s.git.push_target.clone()) else {
            return;
        };
        let Some(repo_id) = parse_remote(&push.url) else {
            return;
        };
        let Some(path) = self.git_lane_path(target) else {
            return;
        };
        let ahead = state
            .and_then(|s| s.git.tracking.as_ref())
            .map_or(0, |t| t.ahead);
        let step = push_first(push.published, ahead);
        let repo = GitHost::global(cx).open(&path);
        let gh = GhHost::global(cx);
        let new = NewPr {
            head: push.branch.clone(),
            base: request.base,
            title: request.title,
            body: request.body,
            draft: request.draft,
            worktree: path.to_path_buf(),
        };
        let poll_repo = repo_id.clone();
        self.spawn_locked_git_work(
            GitLock::Repo,
            target,
            cx,
            move || {
                match step {
                    PushFirst::Publish => repo
                        .push_head(&push.remote, &push.branch, true)
                        .map_err(CreatePrError::Push)?,
                    PushFirst::Update => repo.push().map_err(CreatePrError::Push)?,
                    PushFirst::Nothing => {}
                }
                gh.create_pr(&repo_id, &new).map_err(CreatePrError::Gh)
            },
            move |ws, result, cx| match result {
                Ok(_url) => {
                    PrPoller::nudge(poll_repo, cx);
                    // Deferred: this runs inside the workspace's update, and
                    // closing the dialog re-enters its window.
                    cx.defer(move |cx| {
                        crate::windows::try_update_workspace_window(
                            window,
                            cx,
                            "github.create_pr.close",
                            |window, cx| window.close_dialog(cx),
                        );
                    });
                }
                Err(e) => {
                    let message = e.to_string();
                    let report =
                        ErrorReport::new(crate::surface::strings::error::github_pr_create_failed())
                            .severity(ErrorSeverity::Error)
                            .from_error(&e)
                            .at(file!(), line!())
                            .with_context("repo", redact_home(&path))
                            .dedup("github.create_pr")
                            .build();
                    ws.report_error(report, cx);
                    if let Some(modal) = modal.upgrade() {
                        modal.update(cx, |modal, cx| modal.fail(message, cx));
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_github_has_not_seen_is_pushed() {
        assert_eq!(push_first(false, 0), PushFirst::Publish);
        assert_eq!(push_first(false, 3), PushFirst::Publish);
        assert_eq!(push_first(true, 0), PushFirst::Nothing);
        assert_eq!(push_first(true, 2), PushFirst::Update);
    }
}
