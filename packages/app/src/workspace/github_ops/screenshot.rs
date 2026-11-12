//! Pull-request fixtures for captures; no `gh` or `git` process runs.

use daruda_github::availability::GhAvailability;
use daruda_github::{CheckRollup, FailedCheck, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::Context;

use crate::github::{PrLink, PrPoller};
use crate::lane::git::PushTarget;
use crate::workspace::Workspace;

const HOST: &str = "github.com";

fn repo() -> RepoId {
    RepoId {
        host: HOST.into(),
        owner: "daruda".into(),
        name: "daruda".into(),
    }
}

fn pr(number: u64, title: &str, state: PrState, checks: CheckRollup) -> PrSummary {
    let failed = match checks {
        CheckRollup::Failing { failed } => (0..failed)
            .map(|i| FailedCheck {
                name: format!("build ({i})"),
                details_url: None,
                job: None,
            })
            .collect(),
        _ => Vec::new(),
    };
    PrSummary {
        number,
        url: format!("https://github.com/daruda/daruda/pull/{number}"),
        title: title.into(),
        state,
        head_branch: "sidebar-sizing".into(),
        cross_repo: false,
        checks,
        failed,
        review: None,
        updated_at: String::new(),
    }
}

impl Workspace {
    fn seed_push_target_for_shot(&mut self, target: LaneRef, branch: &str) {
        self.lane_scoped_mut(target).git.push_target = Some(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:daruda/daruda.git".into(),
            branch: branch.into(),
            published: true,
        });
    }

    /// The active lane pushing to GitHub: with a PR whose checks fail, or with
    /// `gh` missing.
    pub(in crate::workspace) fn seed_pr_for_shot(&mut self, linked: bool, cx: &mut Context<Self>) {
        let active = self.active;
        self.seed_push_target_for_shot(active, "sidebar-sizing");
        let availability = if linked {
            GhAvailability::Ready
        } else {
            GhAvailability::Missing
        };
        PrPoller::seed_for_shot(HOST, availability, cx);
        if linked {
            self.lane_scoped_mut(active).pr = Some(PrLink {
                repo: repo(),
                pr: pr(
                    482,
                    "Adjust sidebar presentation for narrow docks",
                    PrState::Open,
                    CheckRollup::Failing { failed: 2 },
                ),
            });
        }
        cx.notify();
    }

    /// One PR per state on the active project's lanes, for the worktree list.
    pub(in crate::workspace) fn seed_lane_prs_for_shot(&mut self, cx: &mut Context<Self>) {
        let project = self.active.project;
        let lanes: Vec<_> = self
            .project_for(project)
            .map(|p| p.lanes.iter().map(|lane| lane.id).collect())
            .unwrap_or_default();
        let states = [
            (PrState::Open, CheckRollup::Failing { failed: 1 }),
            (PrState::Draft, CheckRollup::Pending),
            (PrState::Open, CheckRollup::Passing),
            (PrState::Merged, CheckRollup::Passing),
        ];
        for (i, (lane, (state, checks))) in lanes.into_iter().zip(states).enumerate() {
            let target = LaneRef { project, lane };
            self.seed_push_target_for_shot(target, "sidebar-sizing");
            let number = 480 + i as u64;
            self.lane_scoped_mut(target).pr = Some(PrLink {
                repo: repo(),
                pr: pr(number, "Lane PR", state, checks),
            });
        }
        PrPoller::seed_for_shot(HOST, GhAvailability::Ready, cx);
        cx.notify();
    }
}
