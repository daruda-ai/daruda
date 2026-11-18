//! Opening a pull request for a lane: push HEAD to where the branch pushes,
//! then `gh pr create`, under the repository lock so Push and Fetch wait.
//!
//! Where to push is read again inside the lock, not taken from the cached
//! push target: the branch may have moved or gained commits since.

use std::path::PathBuf;

use daruda_github::availability::GhAvailability;
use daruda_github::remote_parse::parse_remote;
use daruda_github::{GhError, NewPr, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::system_info::redact_home;
use daruda_store::project::LaneRef;
use gpui::{AnyWindowHandle, Context, WeakEntity, Window};

use super::create_modal::CreatePrModal;
use super::lane_pr::PendingPr;
use crate::gh_host::GhHost;
use crate::git_host::GitHost;
use crate::github::{DraftStore, PrPoller};
use crate::lane::git::{GitError, PushTarget};
use crate::surface::strings::error as s;
use crate::ui::WindowExt as _;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::LaneScoped;
use crate::workspace::left_dock::git_ops::lock::GitLock;

/// What the modal hands over; the lane supplies the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreatePrRequest {
    pub title: String,
    pub body: String,
    /// `None` lets GitHub pick the repository's default branch.
    pub base: Option<String>,
    pub draft: bool,
    /// Where the form said the branch pushes. A different target read
    /// under the lock stops the push.
    pub expected: super::create_modal::summary::Expected,
}

/// What creating ended in, short of a failure.
#[derive(Debug)]
enum CreateOutcome {
    Created {
        repo: RepoId,
        head: String,
        url: String,
    },
    /// The branch already has an open pull request: created before (an
    /// answer lost, a timeout) or elsewhere. Offered, never created twice.
    Exists(super::lane_pr::CreatedPr),
}

#[derive(Debug)]
pub(crate) enum CreatePrError {
    /// Detached HEAD, no remote, or a remote that is not a GitHub repository.
    NoTarget,
    /// The branch pushes to the PR's own base.
    HeadIsBase(String),
    /// The branch was made to review someone else's pull request.
    ReviewBranch,
    /// The branch now pushes somewhere other than the form showed.
    TargetChanged,
    Read(GitError),
    Push(GitError),
    /// Pushed; creating the pull request then failed.
    Gh(GhError),
    /// Whether it already has one could not be asked: nothing is pushed or
    /// created on a guess.
    CheckFailed(GhError),
}

impl std::fmt::Display for CreatePrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoTarget => write!(f, "the branch has no GitHub remote to push to"),
            Self::HeadIsBase(branch) => {
                write!(f, "the branch pushes to {branch}, the pull request's base")
            }
            Self::ReviewBranch => write!(f, "the branch reviews another pull request"),
            Self::TargetChanged => write!(f, "the branch's push target changed"),
            Self::Read(e) => write!(f, "read where the branch pushes: {e}"),
            Self::Push(e) => write!(f, "push before creating the pull request: {e}"),
            Self::Gh(e) => write!(f, "pushed, then creating failed: {e}"),
            Self::CheckFailed(e) => write!(f, "could not check for an existing pull request: {e}"),
        }
    }
}

impl std::error::Error for CreatePrError {}

impl CreatePrError {
    /// What the form says: daruda's own reasons translated, git's and gh's
    /// words kept as they came.
    fn form_message(&self) -> String {
        match self {
            Self::NoTarget => s::github_pr_no_remote(),
            Self::HeadIsBase(branch) => s::github_pr_head_is_base(branch),
            Self::ReviewBranch => s::github_pr_review_branch(),
            Self::TargetChanged => s::github_pr_target_changed(),
            Self::Read(e) => format!("{}: {e}", s::github_pr_create_failed()),
            Self::Push(e) => format!("{}: {e}", s::github_pr_push_failed()),
            // A timeout leaves it unknown whether GitHub created it.
            Self::Gh(GhError::Timeout(_)) => s::github_pr_create_uncertain(),
            Self::Gh(e) => format!("{}: {e}", s::github_pr_pushed_create_failed()),
            Self::CheckFailed(e) => format!("{}: {e}", s::github_pr_check_failed()),
        }
    }
}

/// A base as `gh --base` takes it: the bare branch, without the push
/// remote's prefix (`origin/main` names `main` on that remote).
fn strip_remote<'a>(base: &'a str, remote: &str) -> &'a str {
    base.strip_prefix(remote)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(base)
}

/// A lane's recorded base as a default for the form: a branch, never a
/// commit, which no PR can target.
fn base_branch(base: &str, remote: &str) -> Option<String> {
    let base = strip_remote(base, remote);
    let commit_like = (7..=40).contains(&base.len()) && base.chars().all(|c| c.is_ascii_hexdigit());
    (!base.is_empty() && !commit_like).then(|| base.to_owned())
}

/// What Create PR does, decided before anything is pushed.
#[derive(Debug, PartialEq, Eq)]
struct PrPlan {
    repo: RepoId,
    /// `-u` only for a branch with no upstream yet: an existing one (in a
    /// fork workflow, the repository it came from) stays.
    set_upstream: bool,
    new: NewPr,
}

/// Plan the PR for a branch pushing to `push`. `default_base` is the
/// repository's default branch when an answer has named it: with no base
/// typed, GitHub targets that one, so the head must not be it either.
fn plan_pr(
    push: PushTarget,
    request: CreatePrRequest,
    default_base: Option<&str>,
    worktree: PathBuf,
) -> Result<PrPlan, CreatePrError> {
    // Pushing a review branch would copy a fork's head into this
    // repository and open a second pull request for it.
    if push.review.is_some() {
        return Err(CreatePrError::ReviewBranch);
    }
    let repo = parse_remote(&push.url).ok_or(CreatePrError::NoTarget)?;
    let base = request
        .base
        .map(|base| strip_remote(&base, &push.remote).to_owned());
    if base.as_deref().or(default_base) == Some(push.branch.as_str()) {
        return Err(CreatePrError::HeadIsBase(push.branch));
    }
    Ok(PrPlan {
        repo,
        set_upstream: !push.published && !push.has_upstream,
        new: NewPr {
            head: push.branch,
            base,
            title: request.title,
            body: request.body,
            draft: request.draft,
            worktree,
        },
    })
}

impl Workspace {
    /// The base a new PR targets by default: the lane's own base, else the
    /// repository's default branch, else nothing (GitHub's choice).
    fn default_pr_base(&self, lane: LaneRef, cx: &gpui::App) -> Option<String> {
        let (repo, push) = self.lane_scoped.get(&lane)?.github_push()?;
        self.lane_for(lane)
            .and_then(|lane| lane.base_ref.as_deref())
            .and_then(|base| base_branch(base, &push.remote))
            .or_else(|| PrPoller::default_branch(&repo, cx))
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
            fail_modal(modal, s::github_pr_lane_gone(), cx);
            return;
        };
        // With no base named, GitHub picks the default branch; guard against
        // that one too when an answer has told us its name.
        let default_base = self
            .lane_scoped
            .get(&lane)
            .and_then(LaneScoped::github_push)
            .and_then(|(repo, _)| PrPoller::default_branch(&repo, cx));
        // The draft to drop once created, under the account signed in now.
        let creation = self
            .lane_scoped
            .get(&lane)
            .and_then(LaneScoped::github_push)
            .and_then(|(repo, push)| DraftStore::creation_owner(&repo, &push.branch, cx));
        let repo = GitHost::global(cx).open(&path);
        let gh = GhHost::global(cx);
        let report_path = path.clone();
        self.spawn_locked_git_work(
            GitLock::Repo,
            lane,
            cx,
            move || {
                let push = repo.push_target().map_err(CreatePrError::Read)?;
                if !request.expected.holds(push.as_ref()) {
                    return Err(CreatePrError::TargetChanged);
                }
                let push = push.ok_or(CreatePrError::NoTarget)?;
                let remote = push.remote.clone();
                let plan = plan_pr(push, request, default_base.as_deref(), path)?;
                // Before any push: a pull request this branch already has
                // (an earlier create whose answer was lost, or one opened
                // elsewhere) is offered instead of a second one; one that
                // cannot be checked for stops here. The same pull requests
                // count as the poller links: this repository's, on this
                // head, open or draft. A second pull request from the head
                // to another base is not offered: the answer has no base.
                let found = gh
                    .prs_for_branches(&plan.repo, std::slice::from_ref(&plan.new.head), &[])
                    .map_err(CreatePrError::CheckFailed)?;
                if let Some(open) = found
                    .by_branch
                    .get(&plan.new.head)
                    .and_then(|prs| daruda_github::link::open_for(&plan.new.head, prs))
                    && let Some(created) =
                        super::lane_pr::CreatedPr::of(open.url.clone(), &plan.new.head)
                {
                    return Ok(CreateOutcome::Exists(created));
                }
                // An explicit refspec to the branch the PR names: a no-op when
                // the remote is already at HEAD, so it never skips commits a
                // cached ahead count missed.
                repo.push_head(&remote, &plan.new.head, plan.set_upstream)
                    .map_err(CreatePrError::Push)?;
                let url = gh
                    .create_pr(&plan.repo, &plan.new)
                    .map_err(CreatePrError::Gh)?;
                Ok::<_, CreatePrError>(CreateOutcome::Created {
                    repo: plan.repo,
                    head: plan.new.head,
                    url,
                })
            },
            move |ws, result, cx| match result {
                Ok(CreateOutcome::Created {
                    repo: repo_id,
                    head,
                    url,
                }) => {
                    // Published: the fields kept for the branch are done with,
                    // under the account that wrote them and the one now.
                    let now = DraftStore::creation_owner(&repo_id, &head, cx);
                    for owner in creation.iter().chain(now.iter()) {
                        DraftStore::set_creation(owner, None, cx);
                    }
                    // Kept until linked: the strip opens it rather than offer
                    // Create PR again. One with no page to open holds Create
                    // PR only when a poll can link it soon; a host in backoff
                    // would hold the button for its pause.
                    let soon = PrPoller::availability(repo_id.host(), cx) == GhAvailability::Ready;
                    let pending = match super::lane_pr::CreatedPr::of(url, &head) {
                        Some(created) => Some(PendingPr::Named(created)),
                        None => soon.then_some(PendingPr::Unnamed),
                    };
                    if let (Some(state), Some(pending)) = (ws.lane_scoped.get_mut(&lane), pending) {
                        state.pr.created(pending);
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
                Ok(CreateOutcome::Exists(created)) => {
                    if let Some(state) = ws.lane_scoped.get_mut(&lane) {
                        state.pr.created(PendingPr::Named(created.clone()));
                    }
                    exists_modal(modal, created, cx);
                    cx.notify();
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

/// Tell the form the branch already has `created`, to open instead.
fn exists_modal(
    modal: WeakEntity<CreatePrModal>,
    created: super::lane_pr::CreatedPr,
    cx: &mut gpui::App,
) {
    cx.defer(move |cx| {
        if let Some(modal) = modal.upgrade() {
            modal.update(cx, |modal, cx| modal.found_existing(created, cx));
        }
    });
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

    fn push(branch: &str, published: bool, has_upstream: bool) -> PushTarget {
        PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: branch.into(),
            published,
            has_upstream,
            review: None,
        }
    }

    fn request(base: Option<&str>) -> CreatePrRequest {
        CreatePrRequest {
            title: "t".into(),
            body: "b".into(),
            base: base.map(str::to_owned),
            draft: false,
            expected: super::super::create_modal::summary::Expected::Unchecked,
        }
    }

    #[test]
    fn a_plan_targets_the_bare_base_and_refuses_the_head_as_base() {
        // (label, head, typed base, default branch, planned base or None if refused)
        type Row = (
            &'static str,
            &'static str,
            Option<&'static str>,
            Option<&'static str>,
            Option<Option<&'static str>>,
        );
        let cases: [Row; 6] = [
            ("typed base", "feat", Some("main"), None, Some(Some("main"))),
            (
                "prefix stripped",
                "feat",
                Some("origin/main"),
                None,
                Some(Some("main")),
            ),
            (
                "other remote kept",
                "feat",
                Some("upstream/main"),
                None,
                Some(Some("upstream/main")),
            ),
            ("GitHub's choice", "feat", None, Some("main"), Some(None)),
            (
                "typed base is the head",
                "feat",
                Some("origin/feat"),
                None,
                None,
            ),
            (
                "default branch is the head",
                "main",
                None,
                Some("main"),
                None,
            ),
        ];
        for (label, head, base, default, expected) in cases {
            let planned = plan_pr(
                push(head, true, true),
                request(base),
                default,
                PathBuf::new(),
            );
            match expected {
                Some(base) => {
                    let plan = planned.unwrap_or_else(|e| panic!("{label}: {e}"));
                    assert_eq!(plan.new.base.as_deref(), base, "{label}");
                    assert_eq!(plan.new.head, head, "{label}");
                }
                None => assert!(
                    matches!(planned, Err(CreatePrError::HeadIsBase(ref b)) if b == head),
                    "{label}: {planned:?}"
                ),
            }
        }
    }

    #[test]
    fn upstream_is_set_only_for_a_branch_without_one() {
        let cases = [
            ("first push", false, false, true),
            ("already pushed", true, false, false),
            ("tracks another repository", false, true, false),
        ];
        for (label, published, has_upstream, expected) in cases {
            let plan = plan_pr(
                push("feat", published, has_upstream),
                request(None),
                None,
                PathBuf::new(),
            )
            .unwrap();
            assert_eq!(plan.set_upstream, expected, "{label}");
        }
    }

    #[test]
    fn a_review_branch_is_never_pushed_for_a_pull_request() {
        let target = PushTarget {
            review: Some("github.com/o/r#9".into()),
            ..push("pr-9", false, false)
        };
        assert!(matches!(
            plan_pr(target, request(None), None, PathBuf::new()),
            Err(CreatePrError::ReviewBranch)
        ));
    }

    #[test]
    fn a_remote_that_is_not_github_has_no_plan() {
        let mut target = push("feat", true, true);
        target.url = "/srv/git/r.git".into();
        assert!(matches!(
            plan_pr(target, request(None), None, PathBuf::new()),
            Err(CreatePrError::NoTarget)
        ));
    }
}
