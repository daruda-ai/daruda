//! Opening a worktree from a pull request row. A pull request whose branch
//! lives in its own repository gets a worktree on that branch, so pushing
//! updates it; one from a fork gets a local review branch `pr-<N>`, since
//! this repository's remote cannot push to the fork's branch. That branch is
//! marked with the pull request it copies: the Git panel and this page find
//! the pull request by the mark, and neither offers to open another for it.

mod checkout;

use daruda_github::{PrCheckout, PrListItem, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::project::ProjectId;
use gpui::{Context, Window};

use super::list::{self, PrKey};
use crate::lane::git::PushTarget;
use crate::surface::strings::{error, pull_requests as s};
use crate::workspace::Workspace;
use crate::workspace::lane_ops::{
    CreateWorktreePlan, LaneReveal, WorktreeCreate, lane_checkout_path,
};
pub(in crate::workspace) use checkout::CheckoutStage;
use checkout::{CheckedOut, CheckoutError, check_out, description};

/// The pull requests a worktree is being made for, each with how far it has
/// got: `None` until its first stage is told.
pub(in crate::workspace) type Creating = std::collections::BTreeMap<PrKey, Option<CheckoutStage>>;

type AfterCheckout = Box<
    dyn FnOnce(
        &mut Workspace,
        daruda_store::project::LaneRef,
        &mut Window,
        &mut Context<Workspace>,
    ),
>;

impl Workspace {
    /// Go to the worktree working on `item`, making one first if there is
    /// none, in `home` when the person chose the project. Git runs in the
    /// background under the repository's lock; a failure is reported and
    /// leaves nothing registered.
    pub(in crate::workspace) fn create_lane_from_pr(
        &mut self,
        item: PrListItem,
        home: Option<ProjectId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.checkout_pr_for(item, home, None, window, cx);
    }

    pub(super) fn checkout_pr_for(
        &mut self,
        item: PrListItem,
        home: Option<ProjectId>,
        after: Option<AfterCheckout>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lanes = self.pr_lanes();
        if let Some(lane) = list::linked_lane(&item, &lanes) {
            if let Some(after) = after {
                after(self, lane.lane, window, cx)
            } else {
                self.activate_lane(lane.lane, window, cx)
            }
            return;
        }
        let mut after = after;
        if home.is_none() && self.choose_pr_lane_project(&item, &mut after, window, cx) {
            return;
        }

        let Some((project, push)) = self.pr_lane_home(&item.repo, home) else {
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
        // or not yet, and what waits on the checkout gets it as made; one on
        // the review branch's name that was not found as this PR's above is
        // not its review branch.
        if let Some(lane) = list::lane_on_branch(&lanes, project, &checkout.branch) {
            match (checkout.review, after) {
                (None, Some(after)) => after(self, lane.lane, window, cx),
                (None, None) => self.activate_lane(lane.lane, window, cx),
                (Some(_), _) => self.report_review_branch_taken(&checkout.branch, cx),
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
        self.pages
            .pull_requests
            .creating
            .entry(key.clone())
            .or_insert(None);
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
        // Stages reach the button as git gets to them.
        let (stages, mut heard) = futures::channel::mpsc::unbounded();
        let stage_key = key.clone();
        cx.spawn(async move |ws, cx| {
            use futures::StreamExt as _;
            while let Some(stage) = heard.next().await {
                let key = stage_key.clone();
                // SILENT-OK: the window closed; nothing shows the stage.
                let _ = ws.update(cx, |ws, cx| ws.show_checkout_stage(key, stage, cx));
            }
        })
        .detach();
        // A window that closes while git runs takes its in-flight row with
        // it; the pipeline logs a checkout left behind.
        self.spawn_worktree_create(
            create,
            move |repo, new_path| {
                let progress = |stage| {
                    // SILENT-OK: the listener is gone with its window.
                    let _ = stages.unbounded_send(stage);
                };
                check_out(repo, &checkout, &push.url, new_path, &progress)
            },
            move |ws, made, window, cx| {
                ws.pages.pull_requests.creating.remove(&key);
                cx.notify();
                let reveal = if after.is_some() {
                    LaneReveal::Background
                } else {
                    LaneReveal::Foreground
                };
                if let Some(lane) = ws.finish_pr_lane(project, plan, made, reveal, window, cx)
                    && let Some(after) = after
                {
                    after(ws, lane, window, cx);
                }
            },
            window,
            cx,
        )
        .detach();
    }

    /// Clear [`Self::create_lane_from_pr`]'s in-flight row, then register
    /// the worktree git made or report why it did not.
    fn finish_pr_lane(
        &mut self,
        project: ProjectId,
        plan: CreateWorktreePlan,
        made: Result<CheckedOut, CheckoutError>,
        reveal: LaneReveal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<daruda_store::project::LaneRef> {
        match made {
            Err(CheckoutError::Foreign(branch)) => self.report_review_branch_taken(&branch, cx),
            Err(CheckoutError::Occupied(path)) => self.warn_pr_lane(
                error::lane_checkout_occupied(path.display()),
                "pull_requests.checkout_occupied",
                cx,
            ),
            Err(CheckoutError::Git(e)) => self.report_pr_lane_failure(e.to_string(), cx),
            // Opening it again from the pull request takes this worktree.
            Ok(_) if self.project_for(project).is_none() => self.report_pr_lane_failure(
                format!(
                    "the project closed; the worktree is at {}, and opening it again from the pull request takes it",
                    plan.new_path.display()
                ),
                cx,
            ),
            Ok(checked_out) => {
                let branch = plan.branch.clone();
                let lane = self.register_created_lane(plan, project, None, reveal, window, cx);
                if matches!(
                    checked_out,
                    CheckedOut::KeptLocal | CheckedOut::Existing { kept_local: true }
                ) {
                    self.report_kept_local_commits(&branch, cx);
                }
                return Some(lane);
            }
        }
        None
    }

    /// Show how far opening a worktree for `key` has got, while it runs.
    fn show_checkout_stage(&mut self, key: PrKey, stage: CheckoutStage, cx: &mut Context<Self>) {
        if let Some(shown) = self.pages.pull_requests.creating.get_mut(&key) {
            *shown = Some(stage);
            cx.notify();
        }
    }

    /// A repo opened as several projects needs an explicit physical checkout
    /// target before the aggregate page can create a worktree. The answer is
    /// passed on as the worktree's home; the browsed scope stays as it is.
    fn choose_pr_lane_project(
        &mut self,
        item: &PrListItem,
        after: &mut Option<AfterCheckout>,
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
                    project.id,
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
        let after = after.take();
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
                if ws.project_for(project).is_some() {
                    ws.checkout_pr_for(item, Some(project), after, window, cx);
                }
            });
        })
        .detach();
        true
    }

    /// The project a worktree for `repo` is made in, and its push target:
    /// `home` when chosen, else the browsed project's, else the active
    /// project's, else the first that pushes to `repo`.
    pub(super) fn pr_lane_home(
        &self,
        repo: &RepoId,
        home: Option<ProjectId>,
    ) -> Option<(ProjectId, PushTarget)> {
        let selected = home.or_else(|| self.page_project());
        let mut homes = self
            .projects
            .lanes()
            .filter(|(lane, _, _)| selected.is_none_or(|project| lane.project == project))
            .filter_map(|(lane, _, _)| {
                let (pushed, push) = self.lane_scoped.get(&lane)?.github_push()?;
                (pushed == *repo).then(|| (lane.project, push.clone()))
            });
        let first = homes.next()?;
        let active = selected.unwrap_or(self.active.project);
        if first.0 == active {
            return Some(first);
        }
        Some(
            homes
                .find(|(project, _)| *project == active)
                .unwrap_or(first),
        )
    }

    #[track_caller]
    fn report_pr_lane_failure(&mut self, detail: String, cx: &mut Context<Self>) {
        let at = std::panic::Location::caller();
        let report = ErrorReport::new(error::lane_create_failed())
            .severity(ErrorSeverity::Error)
            .at(at.file(), at.line())
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
