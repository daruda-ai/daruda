//! Opening a worktree from a pull request row. A pull request whose branch
//! lives in its own repository gets a worktree on that branch, so pushing
//! updates it; one from a fork gets a local review branch `pr-<N>`, since
//! this repository's remote cannot push to the fork's branch. That branch is
//! marked with the pull request it copies: the Git panel and this page find
//! the pull request by the mark, and neither offers to open another for it.

use daruda_github::remote_parse::parse_remote;
use daruda_github::{PrCheckout, PrListItem, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::project::ProjectId;
use gpui::{Context, Window};

use super::list::{self, PrKey};
use crate::lane::git::{GitError, GitRepo, LaneStart, LocalBranch, PushTarget};
use crate::surface::strings::{error, pull_requests as s};
use crate::workspace::Workspace;
use crate::workspace::lane_ops::{
    CreateWorktreePlan, LaneReveal, WorktreeCreate, lane_checkout_path,
};

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
    push_url: &str,
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
    repo.fetch_ref(
        fetch_source(repo, &checkout.remote, push_url)?,
        &checkout.refspec,
    )?;
    let at_head = match before {
        LocalBranch::Missing | LocalBranch::Contained => true,
        LocalBranch::Ahead => repo.local_branch(branch, head)? == LocalBranch::Contained,
    };
    if at_head {
        repo.move_branch(branch, head)?;
    }
    // Only once the branch exists: its config section goes with it when the
    // branch is deleted, so a failed checkout leaves no orphaned key.
    if before == LocalBranch::Missing && checkout.review.is_none() {
        repo.set_push_remote(branch, &checkout.remote)?;
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

/// Where to fetch the pull request's ref from. The page lists the push
/// repository, so the named remote is used only when its fetch URL names that
/// same repository — then the remote's own transport, proxy and credentials
/// apply. A remote that fetches elsewhere falls back to the push URL.
fn fetch_source<'a>(
    repo: &dyn GitRepo,
    remote: &'a str,
    push_url: &'a str,
) -> Result<&'a str, GitError> {
    let fetches_pushed_repo = repo
        .remote_url(remote)?
        .and_then(|url| parse_remote(&url))
        .is_some_and(|fetched| parse_remote(push_url) == Some(fetched));
    Ok(if fetches_pushed_repo {
        remote
    } else {
        push_url
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
        if self.choose_pr_lane_project(&item, window, cx) {
            return;
        }
        let Some((project, push)) = self.pr_lane_home(&item.repo) else {
            self.report_pr_lane_failure(
                format!("no worktree pushes to {}", item.repo.qualified()),
                cx,
            );
            return;
        };
        let remote = &push.remote;
        let checkout = PrCheckout::of(&item, remote);
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
        let Some(lock) = self.lock_repo(&repo_root) else {
            self.warn_pr_lane(error::lane_create_busy(), "pull_requests.lane_busy", cx);
            return;
        };
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
        let create = WorktreeCreate {
            lock,
            new_path: plan.new_path.clone(),
            dedup: "pull_requests.lane_unregistered",
        };
        // A window that closes while git runs takes its in-flight row with
        // it; the pipeline logs a checkout left behind.
        self.spawn_worktree_create(
            create,
            move |repo, new_path| check_out(repo, &checkout, &push.url, new_path),
            move |ws, made, window, cx| ws.finish_pr_lane(key, project, plan, made, window, cx),
            window,
            cx,
        )
        .detach();
    }

    /// Clear [`Self::create_lane_from_pr`]'s in-flight row, then register
    /// the worktree git made or report why it did not.
    fn finish_pr_lane(
        &mut self,
        key: PrKey,
        project: ProjectId,
        plan: CreateWorktreePlan,
        made: Result<CheckedOut, CheckoutError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

    /// A repo opened as several projects needs an explicit physical checkout
    /// target before the aggregate page can create a worktree.
    fn choose_pr_lane_project(
        &mut self,
        item: &PrListItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.page_scope() != super::super::scope::ProjectScope::All {
            return false;
        }
        let projects: Vec<_> = self
            .projects
            .iter()
            .filter(|project| {
                self.projects.lanes().any(|(lane, _, _)| {
                    lane.project == project.id
                        && self
                            .lane_scoped
                            .get(&lane)
                            .and_then(|state| state.github_push())
                            .is_some_and(|(repo, _)| repo == item.repo)
                })
            })
            // Two checkouts of one repository usually share a name, so each
            // choice also names the directory the worktree would branch from.
            .map(|project| {
                (
                    project.uuid,
                    s::project_choice(&project.name, project.root.display()),
                )
            })
            .collect();
        if projects.len() < 2 {
            return false;
        }
        let mut labels: Vec<_> = projects.iter().map(|(_, label)| label.clone()).collect();
        labels.push(crate::surface::strings::common::btn_cancel());
        let buttons: Vec<_> = labels.iter().map(String::as_str).collect();
        let answer = window.prompt(
            gpui::PromptLevel::Info,
            &crate::surface::strings::pull_requests::choose_worktree_project(),
            None,
            &buttons,
            cx,
        );
        let item = item.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(answer) = answer.await else {
                return;
            };
            let Some((project, _)) = projects.get(answer) else {
                return;
            };
            let project = *project;
            // SILENT-OK: the window can close while choosing the project.
            let _ = this.update_in(cx, |ws, window, cx| {
                if ws.project_by_uuid(project).is_some() {
                    ws.set_page_scope(super::super::scope::ProjectScope::Project(project), cx);
                    ws.create_lane_from_pr(item, window, cx);
                }
            });
        })
        .detach();
        true
    }

    fn pr_lane_home(&self, repo: &RepoId) -> Option<(ProjectId, PushTarget)> {
        let selected = self.page_project();
        let mut homes = self
            .projects
            .lanes()
            .filter(|(lane, _, _)| selected.is_none_or(|project| lane.project == project))
            .filter_map(|(lane, _, _)| {
                let (pushed, push) = self.lane_scoped.get(&lane)?.github_push()?;
                (pushed == *repo).then(|| (lane.project, push.clone()))
            });
        let first = homes.next()?;
        let active = self.page_project().unwrap_or(self.active.project);
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

    #[track_caller]
    fn report_review_branch_taken(&mut self, branch: &str, cx: &mut Context<Self>) {
        self.warn_pr_lane(
            error::lane_review_branch_taken(branch),
            "pull_requests.review_branch_taken",
            cx,
        );
    }

    #[track_caller]
    fn report_kept_local_commits(&mut self, branch: &str, cx: &mut Context<Self>) {
        self.warn_pr_lane(
            error::lane_kept_local_commits(branch),
            "pull_requests.kept_local_commits",
            cx,
        );
    }

    /// A worktree was not made, or not as asked, for a reason the person
    /// can act on. Located at the caller, as an inline report would be.
    #[track_caller]
    fn warn_pr_lane(&mut self, title: String, dedup: &'static str, cx: &mut Context<Self>) {
        let at = std::panic::Location::caller();
        let report = ErrorReport::new(title)
            .severity(ErrorSeverity::Warning)
            .at(at.file(), at.line())
            .dedup(dedup)
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

    const ROOT: &str = "/fake/repo";
    const PUSH_URL: &str = "git@github.com:o/r.git";

    /// What `check_out` asked git to change, the reads left out.
    fn run(repo: FakeRepo, cross_repo: bool) -> (Result<CheckedOut, CheckoutError>, Vec<GitCall>) {
        let git = FakeGit::new().with(repo);
        let checkout = PrCheckout::of(&item(cross_repo, Some("me"), None), "up");
        let made = check_out(
            &*git.open(Path::new(ROOT)),
            &checkout,
            PUSH_URL,
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
            remote: PUSH_URL.into(),
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
                    GitCall::SetPushRemote {
                        repo: ROOT.into(),
                        branch: "fix/scroll".into(),
                        remote: "up".into(),
                    },
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
    fn the_named_remote_is_fetched_only_when_it_fetches_the_pushed_repository() {
        // (label, repository, where the ref is fetched from)
        let cases = [
            (
                "same repository over another transport: the remote",
                FakeRepo::at(ROOT).remote_url("up", "https://github.com/o/r.git"),
                "up",
            ),
            (
                "fetches another repository: the push URL",
                FakeRepo::at(ROOT).remote_url("up", "https://github.com/elsewhere/r.git"),
                PUSH_URL,
            ),
            ("unknown remote: the push URL", FakeRepo::at(ROOT), PUSH_URL),
        ];
        for (label, repo, source) in cases {
            let (made, calls) = run(repo, true);
            assert!(made.is_ok(), "{label}: {made:?}");
            assert!(
                matches!(calls.first(), Some(GitCall::FetchRef { remote, .. }) if remote == source),
                "{label}: {calls:?}"
            );
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
