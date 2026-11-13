//! Opening a worktree from a pull request row. A pull request whose branch
//! lives in its own repository gets a worktree on that branch, so pushing
//! updates it; one from a fork gets a local review branch `pr-<N>`, since
//! this repository's remote cannot push to the fork's branch.

use daruda_github::{PrListItem, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::project::{LaneKind, LaneRef, ProjectId};
use gpui::{Context, Window};

use super::list::{self, PrKey, review_branch};
use crate::lane::git::{GitError, GitRepo};
use crate::surface::strings::{error, pull_requests as s};
use crate::workspace::Workspace;
use crate::workspace::lane_ops::{CreateWorktreePlan, LaneReveal, lane_checkout_path};

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
    /// The fetched ref a new `branch` starts at.
    pub start: String,
}

impl PrCheckout {
    pub(in crate::workspace) fn of(item: &PrListItem, remote: &str) -> Self {
        let number = item.pr.number;
        let (refspec, branch, start) = if item.pr.cross_repo {
            // GitHub keeps every pull request's head in the base repository
            // under `refs/pull/<N>/head`; a private namespace keeps it out of
            // the branch lists.
            let start = format!("refs/daruda/pr/{number}");
            (
                format!("+refs/pull/{number}/head:{start}"),
                review_branch(number),
                start,
            )
        } else {
            let head = &item.pr.head_branch;
            let start = format!("refs/remotes/{remote}/{head}");
            (format!("+refs/heads/{head}:{start}"), head.clone(), start)
        };
        Self {
            remote: remote.to_owned(),
            refspec,
            branch,
            start,
        }
    }
}

/// Fetch the pull request's head, then add the worktree: on the existing
/// local branch when there is one (it may hold the person's own commits),
/// else on a new branch at what was fetched. Blocking; runs off the UI thread.
fn check_out(
    repo: &dyn GitRepo,
    checkout: &PrCheckout,
    new_path: &std::path::Path,
) -> Result<(), GitError> {
    repo.fetch_ref(&checkout.remote, &checkout.refspec)?;
    if repo.has_local_branch(&checkout.branch)? {
        repo.add_lane(new_path, None, Some(&checkout.branch))
    } else {
        repo.add_lane(new_path, Some(&checkout.branch), Some(&checkout.start))
    }
}

/// The lane description of a worktree made for `item`; `None` for one on
/// the pull request's own branch, which says what it is already.
fn description(item: &PrListItem) -> Option<String> {
    // WORKAROUND: the list search does not ask for the head repository's
    // owner, so the author stands in; asking for it changes the search query,
    // whose recorded `gh` fixture has to be captured again against GitHub.
    let owner = item.author.clone().unwrap_or_else(s::author_deleted);
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
        if let Some(lane) = list::linked_lane(&item, &self.pr_page_lanes()) {
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
        // worktree, so a lane on it is the one to go to, linked or not yet.
        if let Some(lane) = self.lane_on_branch(project, &checkout.branch) {
            self.activate_lane(lane, window, cx);
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
                    async move { check_out(&*repo, &checkout, &new_path).map_err(|e| e.to_string()) }
                })
                .await;
            // A window that closed while git ran took its lock and its
            // in-flight row with it; a checkout git made stays on disk.
            // SILENT-OK: nothing is left in a closed window to release or report
            let _ = ws.update_in(cx, |ws, window, cx| {
                ws.finish_pr_lane(key, project, plan, made, window, cx)
            });
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
        made: Result<(), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.release_repo_lock(&plan.repo_root);
        self.pages.pull_requests.creating.remove(&key);
        cx.notify();
        match made {
            Err(detail) => self.report_pr_lane_failure(detail, cx),
            Ok(()) if self.project_for(project).is_none() => self.report_pr_lane_failure(
                format!(
                    "the project closed; the worktree is at {}",
                    plan.new_path.display()
                ),
                cx,
            ),
            Ok(()) => {
                self.register_created_lane(plan, project, None, LaneReveal::Foreground, window, cx);
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

    /// The lane of `project` checked out on `branch`, as its record says.
    fn lane_on_branch(&self, project: ProjectId, branch: &str) -> Option<LaneRef> {
        self.project_for(project)?
            .lanes
            .iter()
            .find(|lane| matches!(&lane.kind, LaneKind::Git { branch: Some(b), .. } if b == branch))
            .map(|lane| LaneRef {
                project,
                lane: lane.id,
            })
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
}

#[cfg(test)]
mod tests {
    use daruda_github::{CheckRollup, PrState, PrSummary};

    use super::*;

    fn item(cross_repo: bool, author: Option<&str>) -> PrListItem {
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
            base_branch: "main".into(),
            comments: 0,
        }
    }

    #[test]
    fn a_pull_request_is_fetched_to_a_name_its_branch_starts_from() {
        // (label, cross_repo, refspec, branch, start)
        let cases = [
            (
                "same repository: its own branch",
                false,
                "+refs/heads/fix/scroll:refs/remotes/up/fix/scroll",
                "fix/scroll",
                "refs/remotes/up/fix/scroll",
            ),
            (
                "fork: a review branch",
                true,
                "+refs/pull/9/head:refs/daruda/pr/9",
                "pr-9",
                "refs/daruda/pr/9",
            ),
        ];
        for (label, cross_repo, refspec, branch, start) in cases {
            let checkout = PrCheckout::of(&item(cross_repo, Some("me")), "up");
            assert_eq!(
                checkout,
                PrCheckout {
                    remote: "up".into(),
                    refspec: refspec.into(),
                    branch: branch.into(),
                    start: start.into(),
                },
                "{label}"
            );
        }
    }

    #[test]
    fn only_a_review_worktree_is_described() {
        assert_eq!(description(&item(false, Some("me"))), None);
        let described = description(&item(true, Some("alice"))).unwrap();
        assert!(described.contains("#9"), "{described}");
        assert!(described.contains("alice:fix/scroll"), "{described}");
    }
}
