//! Opening a pull request for a lane: push HEAD to where the branch pushes,
//! then `gh pr create`, under the repository lock so Push and Fetch wait.
//!
//! Where to push is read again inside the lock, not taken from the cached
//! push target: the branch may have moved or gained commits since.

use daruda_github::remote_parse::parse_remote;
use daruda_github::{GhError, NewPr};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::system_info::redact_home;
use daruda_store::project::LaneRef;
use gpui::{AnyWindowHandle, Context, WeakEntity, Window};

use super::create_modal::CreatePrModal;
use crate::gh_host::GhHost;
use crate::git_host::GitHost;
use crate::github::PrPoller;
use crate::lane::git::GitError;
use crate::surface::strings::error as s;
use crate::ui::WindowExt as _;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::PrWork;
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
    /// The lane was closed before the form was submitted.
    LaneGone,
    /// Detached HEAD, no remote, or a remote that is not a GitHub repository.
    NoTarget,
    /// The branch pushes to the PR's own base.
    HeadIsBase(String),
    Read(GitError),
    Push(GitError),
    Gh(GhError),
}

impl std::fmt::Display for CreatePrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LaneGone => write!(f, "the worktree is no longer open"),
            Self::NoTarget => write!(f, "the branch has no GitHub remote to push to"),
            Self::HeadIsBase(branch) => {
                write!(f, "the branch pushes to {branch}, the pull request's base")
            }
            Self::Read(e) => write!(f, "read where the branch pushes: {e}"),
            Self::Push(e) => write!(f, "push before creating the pull request: {e}"),
            Self::Gh(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CreatePrError {}

impl CreatePrError {
    /// What the form says: daruda's own reasons translated, git's and gh's
    /// words kept as they came.
    fn form_message(&self) -> String {
        match self {
            Self::LaneGone => s::github_pr_lane_gone(),
            Self::NoTarget => s::github_pr_no_remote(),
            Self::HeadIsBase(branch) => s::github_pr_head_is_base(branch),
            Self::Read(_) | Self::Push(_) | Self::Gh(_) => {
                format!("{}: {self}", s::github_pr_create_failed())
            }
        }
    }
}

/// A lane base as `gh --base` takes it: without the push remote's prefix,
/// and not a commit, which no PR can target.
fn base_branch(base: &str, remote: &str) -> Option<String> {
    let base = base.strip_prefix(&format!("{remote}/")).unwrap_or(base);
    let commit_like = (7..=40).contains(&base.len()) && base.chars().all(|c| c.is_ascii_hexdigit());
    (!base.is_empty() && !commit_like).then(|| base.to_owned())
}

impl Workspace {
    /// The base a new PR targets by default: the lane's own base, else the
    /// repository's default branch, else nothing (GitHub's choice).
    fn default_pr_base(&self, lane: LaneRef, cx: &gpui::App) -> Option<String> {
        let push = self.lane_scoped.get(&lane)?.git.push_target.as_ref()?;
        self.lane_for(lane)
            .and_then(|lane| lane.base_ref.as_deref())
            .and_then(|base| base_branch(base, &push.remote))
            .or_else(|| PrPoller::default_branch(&parse_remote(&push.url)?, cx))
    }

    pub(in crate::workspace) fn open_create_pr_modal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lane = self.active;
        let Some(path) = self.git_lane_path(lane) else {
            return;
        };
        let base = self.default_pr_base(lane, cx);
        let workspace = cx.weak_entity();
        let repo = GitHost::global(cx).open(&path);
        crate::workspace::dialog_helpers::open_form_modal(
            crate::surface::strings::git::pr_create_title(),
            None,
            move |window, cx| CreatePrModal::new(workspace, lane, repo, base, window, cx),
            window,
            cx,
        );
    }

    /// Push HEAD, then open the PR for `lane`. `modal` hears every outcome;
    /// on success the dialog in `window` closes and the poller asks at once.
    pub(in crate::workspace) fn create_pr(
        &mut self,
        lane: LaneRef,
        request: CreatePrRequest,
        modal: WeakEntity<CreatePrModal>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        if self.git_lock_held(GitLock::Repo) {
            fail_modal(modal, s::github_git_busy(), cx);
            return;
        }
        let Some(path) = self.git_lane_path(lane) else {
            fail_modal(modal, CreatePrError::LaneGone.form_message(), cx);
            return;
        };
        // With no base named, GitHub picks the default branch; guard against
        // that one too when an answer has told us its name.
        let default_base = self
            .lane_scoped
            .get(&lane)
            .and_then(|state| state.git.push_target.as_ref())
            .and_then(|push| parse_remote(&push.url))
            .and_then(|repo| PrPoller::default_branch(&repo, cx));
        let repo = GitHost::global(cx).open(&path);
        let gh = GhHost::global(cx);
        let report_path = path.clone();
        self.spawn_locked_git_work(
            GitLock::Repo,
            lane,
            cx,
            move || {
                let push = repo
                    .push_target()
                    .map_err(CreatePrError::Read)?
                    .ok_or(CreatePrError::NoTarget)?;
                let repo_id = parse_remote(&push.url).ok_or(CreatePrError::NoTarget)?;
                // A typed base may carry the remote's prefix (`origin/feat`).
                let base = request.base.as_deref().or(default_base.as_deref());
                let base = base.map(|b| b.strip_prefix(&format!("{}/", push.remote)).unwrap_or(b));
                if base == Some(push.branch.as_str()) {
                    return Err(CreatePrError::HeadIsBase(push.branch));
                }
                // An explicit refspec to the branch the PR names: a no-op when
                // the remote is already at HEAD, so it never skips commits a
                // cached ahead count missed.
                // `-u` only for a branch with no upstream yet: an existing one
                // (in a fork workflow, the repository it came from) stays.
                let set_upstream = !push.published && !push.has_upstream;
                repo.push_head(&push.remote, &push.branch, set_upstream)
                    .map_err(CreatePrError::Push)?;
                let new = NewPr {
                    head: push.branch,
                    base: request.base,
                    title: request.title,
                    body: request.body,
                    draft: request.draft,
                    worktree: path,
                };
                gh.create_pr(&repo_id, &new).map_err(CreatePrError::Gh)?;
                Ok(repo_id)
            },
            move |ws, result, cx| match result {
                Ok(repo_id) => {
                    if let Some(state) = ws.lane_scoped.get_mut(&lane) {
                        state.pr_work = PrWork::AwaitingNewPr;
                    }
                    PrPoller::nudge(repo_id, cx);
                    // Deferred: this runs inside the workspace's update, and
                    // closing the dialog re-enters its window. Only this form
                    // closes; if it is gone, whatever replaced it stays.
                    cx.defer(move |cx| {
                        if modal.upgrade().is_none() {
                            return;
                        }
                        crate::windows::try_update_workspace_window(
                            window,
                            cx,
                            "github.create_pr.close",
                            |window, cx| window.close_dialog(cx),
                        );
                    });
                }
                Err(e) => {
                    let report = ErrorReport::new(s::github_pr_create_failed())
                        .severity(ErrorSeverity::Error)
                        .from_error(&e)
                        .at(file!(), line!())
                        .with_context("repo", redact_home(&report_path))
                        .dedup("github.create_pr")
                        .build();
                    ws.report_error(report, cx);
                    fail_modal(modal, e.form_message(), cx);
                }
            },
        );
    }
}

/// Tell the form why it failed. Deferred: callers run inside an update the
/// form's own update must not nest in (G9).
fn fail_modal(modal: WeakEntity<CreatePrModal>, reason: String, cx: &mut gpui::App) {
    cx.defer(move |cx| {
        if let Some(modal) = modal.upgrade() {
            modal.update(cx, |modal, cx| modal.fail(reason, cx));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_base_becomes_a_branch_gh_can_target() {
        let cases = [
            ("origin/main", Some("main")),
            ("main", Some("main")),
            ("feature/x", Some("feature/x")),
            ("upstream/main", Some("upstream/main")),
            ("1a2b3c4d", None),
            ("origin/", None),
        ];
        for (base, expected) in cases {
            assert_eq!(base_branch(base, "origin").as_deref(), expected, "{base}");
        }
    }
}
