//! Opening a worktree from a pull request row. A pull request whose branch
//! lives in its own repository gets a worktree on that branch, so pushing
//! updates it; one from a fork gets a local review branch `pr-<N>`, since
//! this repository's remote cannot push to the fork's branch. That branch is
//! marked with the pull request it copies: the Git panel and this page find
//! the pull request by the mark, and neither offers to open another for it.

use daruda_github::link::ReviewMark;
use daruda_github::{PrListItem, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::project::ProjectId;
use gpui::{Context, Window};

use super::list::{self, PrKey};
use crate::lane::git::{GitError, GitRepo, LaneStart, LocalBranch};
use crate::surface::strings::{error, pull_requests as s};
use crate::workspace::Workspace;
use crate::workspace::lane_ops::{CreateWorktreePlan, LaneReveal, lane_checkout_path};

/// The branch a worktree made to review fork pull request `number` is on.
fn review_branch(number: u64) -> String {
    format!("pr-{number}")
}

/// What git is asked for to put a pull request's head in a worktree,
/// decided before any of it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PrCheckout {
    pub remote: String,
    /// What `git fetch <remote>` is given. It names its destination, so the
    /// fetched commit has a name `git worktree add` can start from: a plain
    /// `git fetch <remote> <head>` only moves `<remote>/<head>` when the
    /// remote's configured refspec covers the branch, which a single-branch
    /// clone's does not, and `FETCH_HEAD` is rewritten by any other fetch.
    pub refspec: String,
    /// The local branch the worktree is on.
    pub branch: String,
    /// The fetched ref `branch` is brought to.
    pub start: String,
    /// For a fork's pull request, the mark its review branch carries.
    pub review: Option<String>,
}

impl PrCheckout {
    pub(in crate::workspace) fn of(item: &PrListItem, remote: &str) -> Self {
        let number = item.pr.number;
        let (refspec, branch, start, review) = if item.pr.cross_repo {
            // GitHub keeps every pull request's head in the base repository
            // under `refs/pull/<N>/head`; a private namespace keeps it out of
            // the branch lists.
            let start = format!("refs/daruda/pr/{number}");
            let mark = ReviewMark {
                repo: item.repo.clone(),
                number,
            };
            (
                format!("+refs/pull/{number}/head:{start}"),
                review_branch(number),
                start,
                Some(mark.encode()),
            )
        } else {
            let head = &item.pr.head_branch;
            let start = format!("refs/remotes/{remote}/{head}");
            (
                format!("+refs/heads/{head}:{start}"),
                head.clone(),
                start,
                None,
            )
        };
        Self {
            remote: remote.to_owned(),
            refspec,
            branch,
            start,
            review,
        }
    }
}

/// Where the worktree's branch was left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckedOut {
    /// At the pull request's fetched head: made there, or moved there from a
    /// commit that head, or the one fetched before it, already held.
    AtHead,
    /// Where it was: it has commits of its own the head lacks, which moving
    /// it would drop.
    KeptLocal,
}

#[derive(Debug)]
enum CheckoutError {
    /// A branch of the review branch's name exists that was not made to
    /// review this pull request; it is the person's, and is left alone.
    Foreign(String),
    Git(GitError),
}

impl std::fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Foreign(branch) => write!(f, "{branch} exists and reviews no such pull request"),
            Self::Git(e) => e.fmt(f),
        }
    }
}

impl From<GitError> for CheckoutError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

/// Fetch the pull request's head, bring the local branch to it unless that
/// would drop commits of the person's, then add the worktree on the branch.
/// Blocking; runs off the UI thread.
fn check_out(
    repo: &dyn GitRepo,
    checkout: &PrCheckout,
    new_path: &std::path::Path,
) -> Result<CheckedOut, CheckoutError> {
    let (branch, head) = (checkout.branch.as_str(), checkout.start.as_str());
    // Measured before the fetch as well: after a force push the new head no
    // longer holds the old one, yet a branch inside the old one has nothing
    // of the person's to lose.
    let before = repo.local_branch(branch, head)?;
    if let Some(mark) = &checkout.review
        && before != LocalBranch::Missing
        && repo.review_mark(branch)?.as_ref() != Some(mark)
    {
        return Err(CheckoutError::Foreign(branch.to_owned()));
    }
    repo.fetch_ref(&checkout.remote, &checkout.refspec)?;
    let at_head = match before {
        LocalBranch::Missing | LocalBranch::Contained => true,
        LocalBranch::Ahead => repo.local_branch(branch, head)? == LocalBranch::Contained,
    };
    if at_head {
        repo.move_branch(branch, head)?;
    }
    if let Some(mark) = &checkout.review
        && before == LocalBranch::Missing
    {
        repo.set_review_mark(branch, mark)?;
    }
    repo.add_lane(new_path, LaneStart::existing_branch(branch))?;
    Ok(if at_head {
        CheckedOut::AtHead
    } else {
        CheckedOut::KeptLocal
    })
}

/// The lane description of a worktree made for `item`; `None` for one on
/// the pull request's own branch, which says what it is already.
fn description(item: &PrListItem) -> Option<String> {
    // GitHub answers no head owner once the fork is deleted; the author,
    // who opened the PR from it, is the closest name left.
    let owner = item
        .head_owner
        .clone()
        .or_else(|| item.author.clone())
        .unwrap_or_else(s::author_deleted);
    item.pr
        .cross_repo
        .then(|| s::review_lane_description(item.pr.number, owner, &item.pr.head_branch))
}

impl Workspace {
    /// Go to the worktree working on `item`, making one first if there is
    /// none. Git runs in the background under the repository's lock; a
    /// failure is reported and leaves nothing registered.
    pub(in crate::workspace) fn create_lane_from_pr(
        &mut self,
        item: PrListItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lanes = self.pr_page_lanes();
        if let Some(lane) = list::linked_lane(&item, &lanes) {
            self.activate_lane(lane.lane, window, cx);
            return;
        }
        let Some((project, remote)) = self.pr_lane_home(&item.repo) else {
            self.report_pr_lane_failure(
                format!("no worktree pushes to {}", item.repo.qualified()),
                cx,
            );
            return;
        };
        let checkout = PrCheckout::of(&item, &remote);
        // `git worktree add` refuses a branch checked out in another
        // worktree. A lane on the PR's own branch is the one to go to, linked
        // or not yet; one on the review branch's name that was not found as
        // this PR's above is not its review branch.
        if let Some(lane) = list::lane_on_branch(&lanes, project, &checkout.branch) {
            match checkout.review {
                None => self.activate_lane(lane.lane, window, cx),
                Some(_) => self.report_review_branch_taken(&checkout.branch, cx),
            }
            return;
        }
        let Some(repo_root) = self.git_repo_root_for_project(project) else {
            self.report_pr_lane_failure(format!("project {project} is not a git repository"), cx);
            return;
        };
        if !self.acquire_repo_lock(&repo_root) {
            let report = ErrorReport::new(error::lane_create_busy())
                .severity(ErrorSeverity::Warning)
                .at(file!(), line!())
                .dedup("pull_requests.lane_busy")
                .build();
            self.report_error(report, cx);
            return;
        }
        let key: PrKey = (item.repo.clone(), item.pr.number);
        self.pages.pull_requests.creating.insert(key.clone());
        cx.notify();

        let plan = CreateWorktreePlan {
            new_path: lane_checkout_path(&repo_root, &checkout.branch),
            branch: checkout.branch.clone(),
            repo_root,
            // What the worktree is compared against and a new PR targets:
            // the pull request's base, not the head it starts at.
            base_ref: Some(format!("{remote}/{}", item.base_branch)),
            description: description(&item),
            session_host: None,
        };
        let repo = crate::git_host::GitHost::global(cx).open(&plan.repo_root);
        cx.spawn_in(window, async move |ws, cx| {
            let made = cx
                .background_executor()
                .spawn({
                    let new_path = plan.new_path.clone();
                    async move { check_out(&*repo, &checkout, &new_path) }
                })
                .await;
            let created = made.is_ok().then(|| plan.new_path.clone());
            let finished = ws.update_in(cx, |ws, window, cx| {
                ws.finish_pr_lane(key, project, plan, made, window, cx)
            });
            // A window that closed while git ran took its lock and its
            // in-flight row with it. A checkout git made stays on disk,
            // registered nowhere, so the log is the only place it is named.
            if let (Err(e), Some(path)) = (finished, created) {
                LogWriter::log(
                    ErrorReport::new("PR worktree was made but its workspace closed first")
                        .severity(ErrorSeverity::Warning)
                        .with_context("path", path.display().to_string())
                        .with_context("error", e.to_string())
                        .at(file!(), line!())
                        .dedup("pull_requests.lane_unregistered")
                        .build(),
                );
            }
        })
        .detach();
    }

    /// Release what [`Self::create_lane_from_pr`] held, then register the
    /// worktree git made or report why it did not.
    fn finish_pr_lane(
        &mut self,
        key: PrKey,
        project: ProjectId,
        plan: CreateWorktreePlan,
        made: Result<CheckedOut, CheckoutError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.release_repo_lock(&plan.repo_root);
        self.pages.pull_requests.creating.remove(&key);
        cx.notify();
        match made {
            Err(CheckoutError::Foreign(branch)) => self.report_review_branch_taken(&branch, cx),
            Err(CheckoutError::Git(e)) => self.report_pr_lane_failure(e.to_string(), cx),
            Ok(_) if self.project_for(project).is_none() => self.report_pr_lane_failure(
                format!(
                    "the project closed; the worktree is at {}",
                    plan.new_path.display()
                ),
                cx,
            ),
            Ok(checked_out) => {
                let branch = plan.branch.clone();
                self.register_created_lane(plan, project, None, LaneReveal::Foreground, window, cx);
                if checked_out == CheckedOut::KeptLocal {
                    self.report_kept_local_commits(&branch, cx);
                }
            }
        }
    }

    /// The project to make `repo`'s pull request worktrees in, and the remote
    /// its lanes reach `repo` through: the active project's when it pushes
    /// there, else the first that does.
    fn pr_lane_home(&self, repo: &RepoId) -> Option<(ProjectId, String)> {
        let mut homes = self.projects.lanes().filter_map(|(lane, _, _)| {
            let (pushed, push) = self.lane_scoped.get(&lane)?.github_push()?;
            (pushed == *repo).then(|| (lane.project, push.remote.clone()))
        });
        let first = homes.next()?;
        let active = self.active.project;
        if first.0 == active {
            return Some(first);
        }
        Some(
            homes
                .find(|(project, _)| *project == active)
                .unwrap_or(first),
        )
    }

    fn report_pr_lane_failure(&mut self, detail: String, cx: &mut Context<Self>) {
        let report = ErrorReport::new(error::lane_create_failed())
            .severity(ErrorSeverity::Error)
            .at(file!(), line!())
            .with_context("detail", detail)
            .dedup("pull_requests.lane_create")
            .build();
        self.report_error(report, cx);
    }

    fn report_review_branch_taken(&mut self, branch: &str, cx: &mut Context<Self>) {
        let report = ErrorReport::new(error::lane_review_branch_taken(branch))
            .severity(ErrorSeverity::Warning)
            .at(file!(), line!())
            .dedup("pull_requests.review_branch_taken")
            .build();
        self.report_error(report, cx);
    }

    fn report_kept_local_commits(&mut self, branch: &str, cx: &mut Context<Self>) {
        let report = ErrorReport::new(error::lane_kept_local_commits(branch))
            .severity(ErrorSeverity::Warning)
            .at(file!(), line!())
            .dedup("pull_requests.kept_local_commits")
            .build();
        self.report_error(report, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use daruda_github::{CheckRollup, PrState, PrSummary};

    use super::*;
    use crate::lane::git::{FakeGit, FakeRepo, Git as _, GitCall};

    fn item(cross_repo: bool, author: Option<&str>, head_owner: Option<&str>) -> PrListItem {
        PrListItem {
            repo: RepoId::new("github.com", "o", "r"),
            pr: PrSummary {
                number: 9,
                url: "https://github.com/o/r/pull/9".into(),
                title: "t".into(),
                state: PrState::Open,
                head_branch: "fix/scroll".into(),
                cross_repo,
                checks: CheckRollup::None,
                failed: Vec::new(),
                review: None,
                updated_at: String::new(),
            },
            author: author.map(str::to_owned),
            head_owner: head_owner.map(str::to_owned),
            base_branch: "main".into(),
            comments: 0,
        }
    }

    #[test]
    fn a_pull_request_is_fetched_to_a_name_its_branch_starts_from() {
        // (label, cross_repo, refspec, branch, start, review mark)
        let cases = [
            (
                "same repository: its own branch",
                false,
                "+refs/heads/fix/scroll:refs/remotes/up/fix/scroll",
                "fix/scroll",
                "refs/remotes/up/fix/scroll",
                None,
            ),
            (
                "fork: a review branch marked with its PR",
                true,
                "+refs/pull/9/head:refs/daruda/pr/9",
                "pr-9",
                "refs/daruda/pr/9",
                Some("github.com/o/r#9"),
            ),
        ];
        for (label, cross_repo, refspec, branch, start, review) in cases {
            let checkout = PrCheckout::of(&item(cross_repo, Some("me"), None), "up");
            assert_eq!(
                checkout,
                PrCheckout {
                    remote: "up".into(),
                    refspec: refspec.into(),
                    branch: branch.into(),
                    start: start.into(),
                    review: review.map(str::to_owned),
                },
                "{label}"
            );
        }
    }

    const ROOT: &str = "/fake/repo";

    /// What `check_out` asked git to change, the reads left out.
    fn run(repo: FakeRepo, cross_repo: bool) -> (Result<CheckedOut, CheckoutError>, Vec<GitCall>) {
        let git = FakeGit::new().with(repo);
        let checkout = PrCheckout::of(&item(cross_repo, Some("me"), None), "up");
        let made = check_out(
            &*git.open(Path::new(ROOT)),
            &checkout,
            Path::new("/fake/lane"),
        );
        (made, git.calls())
    }

    fn fetch(cross_repo: bool) -> GitCall {
        let refspec = if cross_repo {
            "+refs/pull/9/head:refs/daruda/pr/9"
        } else {
            "+refs/heads/fix/scroll:refs/remotes/up/fix/scroll"
        };
        GitCall::FetchRef {
            repo: ROOT.into(),
            remote: "up".into(),
            refspec: refspec.into(),
        }
    }

    fn moved(branch: &str, head: &str) -> GitCall {
        GitCall::MoveBranch {
            repo: ROOT.into(),
            branch: branch.into(),
            head: head.into(),
        }
    }

    fn added(branch: &str) -> GitCall {
        GitCall::AddLane {
            repo: ROOT.into(),
            new_path: "/fake/lane".into(),
            start: LaneStart::existing_branch(branch),
        }
    }

    const MARK: &str = "github.com/o/r#9";
    const REVIEW_HEAD: &str = "refs/daruda/pr/9";
    const SAME_HEAD: &str = "refs/remotes/up/fix/scroll";

    #[test]
    fn a_branch_is_brought_to_the_fetched_head_unless_it_has_commits_of_its_own() {
        let mark = GitCall::SetReviewMark {
            repo: ROOT.into(),
            branch: "pr-9".into(),
            mark: MARK.into(),
        };
        let at = FakeRepo::at(ROOT);
        // (label, repository, cross_repo, outcome, calls)
        let cases = vec![
            (
                "new review branch: made at the head and marked",
                at.clone(),
                true,
                CheckedOut::AtHead,
                vec![fetch(true), moved("pr-9", REVIEW_HEAD), mark, added("pr-9")],
            ),
            (
                "an old review branch is reset to the fresh head",
                at.clone()
                    .local_branches(["pr-9"])
                    .review_mark("pr-9", MARK),
                true,
                CheckedOut::AtHead,
                vec![fetch(true), moved("pr-9", REVIEW_HEAD), added("pr-9")],
            ),
            (
                "a review branch with commits of its own is kept",
                at.clone()
                    .ahead_branches(["pr-9"])
                    .review_mark("pr-9", MARK),
                true,
                CheckedOut::KeptLocal,
                vec![fetch(true), added("pr-9")],
            ),
            (
                "same repository, new: made at the head",
                at.clone(),
                false,
                CheckedOut::AtHead,
                vec![
                    fetch(false),
                    moved("fix/scroll", SAME_HEAD),
                    added("fix/scroll"),
                ],
            ),
            (
                "same repository, behind: fast-forwarded",
                at.clone().local_branches(["fix/scroll"]),
                false,
                CheckedOut::AtHead,
                vec![
                    fetch(false),
                    moved("fix/scroll", SAME_HEAD),
                    added("fix/scroll"),
                ],
            ),
            (
                "same repository, with unpushed commits: kept",
                at.clone().ahead_branches(["fix/scroll"]),
                false,
                CheckedOut::KeptLocal,
                vec![fetch(false), added("fix/scroll")],
            ),
        ];
        for (label, repo, cross_repo, outcome, calls) in cases {
            let (made, logged) = run(repo, cross_repo);
            assert_eq!(made.ok(), Some(outcome), "{label}");
            assert_eq!(logged, calls, "{label}");
        }
    }

    #[test]
    fn a_branch_of_the_review_branchs_name_made_otherwise_is_left_alone() {
        // (label, repository)
        let cases = [
            ("unmarked", FakeRepo::at(ROOT).local_branches(["pr-9"])),
            (
                "marked for another pull request",
                FakeRepo::at(ROOT)
                    .local_branches(["pr-9"])
                    .review_mark("pr-9", "github.com/fork/r#9"),
            ),
        ];
        for (label, repo) in cases {
            let (made, calls) = run(repo, true);
            assert!(
                matches!(made, Err(CheckoutError::Foreign(ref b)) if b == "pr-9"),
                "{label}: {made:?}"
            );
            assert!(
                calls.is_empty(),
                "{label}: nothing fetched or moved: {calls:?}"
            );
        }
    }

    #[test]
    fn only_a_review_worktree_is_described_by_its_forks_owner() {
        assert_eq!(description(&item(false, Some("me"), Some("o"))), None);
        // (label, author, head owner, the `owner:branch` described)
        let cases = [
            (
                "the fork's owner, not the author",
                Some("alice"),
                Some("acme"),
                "acme:fix/scroll",
            ),
            (
                "a deleted fork falls back to the author",
                Some("alice"),
                None,
                "alice:fix/scroll",
            ),
        ];
        for (label, author, head_owner, expected) in cases {
            let described = description(&item(true, author, head_owner)).unwrap();
            assert!(described.contains("#9"), "{label}: {described}");
            assert!(described.contains(expected), "{label}: {described}");
        }
    }
}
