//! Pull-request fixtures for captures. Every capture starts on an empty
//! `FakeGh` (`startup::inspection::before_globals`); these scenarios replace
//! it with one that answers like the seeds, so the running poller does not
//! overwrite what was seeded.

use std::sync::Arc;

use daruda_github::availability::{GhAuth, GhAvailability};
use daruda_github::fake::FakeGh;
use daruda_github::{CheckRollup, FailedCheck, GITHUB_HOST, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::Context;

use crate::gh_host::GhHost;
use crate::github::{PrLink, PrLookup, PrPoller};
use crate::lane::git::PushTarget;
use crate::workspace::Workspace;

fn repo() -> RepoId {
    RepoId::new(GITHUB_HOST, "daruda", "daruda")
}

fn pr(number: u64, head: &str, title: &str, state: PrState, checks: CheckRollup) -> PrSummary {
    let failed = match checks {
        CheckRollup::Failing { failed } => (0..failed.unwrap_or(0))
            .map(|i| FailedCheck {
                workflow: None,
                event: None,
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
        head_branch: head.into(),
        cross_repo: false,
        checks,
        failed,
        review: None,
        updated_at: String::new(),
    }
}

/// What the active lane's PR looks like in a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PrShot {
    /// A PR whose checks fail.
    Linked,
    /// GitHub answers; the branch has no PR yet.
    NoPr,
    /// `gh` is not installed.
    GhMissing,
}

/// Answer for GitHub the way the seeds say it is.
fn install_fake_gh(auth: GhAuth, prs: Vec<PrSummary>, cx: &mut gpui::App) {
    let gh = FakeGh::new();
    gh.set_auth(GITHUB_HOST, auth);
    gh.set_prs(&repo(), prs);
    GhHost::set_global(Arc::new(gh), cx);
}

impl Workspace {
    /// Written directly rather than through `set_push_target`: that would
    /// nudge the poller before the scripted answers are installed.
    fn seed_push_target_for_shot(&mut self, target: LaneRef, branch: &str) {
        self.lane_scoped_mut(target).git.push_target = Some(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:daruda/daruda.git".into(),
            branch: branch.into(),
            published: true,
            has_upstream: false,
        });
    }

    /// The active lane pushing to GitHub, its PR as `shot` says.
    pub(in crate::workspace) fn seed_pr_for_shot(&mut self, shot: PrShot, cx: &mut Context<Self>) {
        const BRANCH: &str = "sidebar-sizing";
        let active = self.active;
        self.seed_push_target_for_shot(active, BRANCH);
        let linked = pr(
            482,
            BRANCH,
            "Adjust sidebar presentation for narrow docks",
            PrState::Open,
            CheckRollup::Failing { failed: Some(2) },
        );
        let (auth, availability, prs) = match shot {
            PrShot::Linked => (GhAuth::Ready, GhAvailability::Ready, vec![linked.clone()]),
            PrShot::NoPr => (GhAuth::Ready, GhAvailability::Ready, Vec::new()),
            PrShot::GhMissing => (GhAuth::Missing, GhAvailability::Missing, Vec::new()),
        };
        install_fake_gh(auth, prs, cx);
        PrPoller::seed_for_shot(GITHUB_HOST, availability, cx);
        let lookup = match shot {
            PrShot::Linked => PrLookup::Linked(Box::new(PrLink {
                repo: repo(),
                pr: linked,
            })),
            PrShot::NoPr => PrLookup::NoPr,
            PrShot::GhMissing => PrLookup::Unknown,
        };
        self.seed_pr_lookup_for_shot(active, lookup, cx);
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
            (PrState::Open, CheckRollup::Failing { failed: Some(1) }),
            (PrState::Draft, CheckRollup::Pending),
            (PrState::Open, CheckRollup::Passing),
            (PrState::Merged, CheckRollup::Passing),
        ];
        let mut prs = Vec::new();
        for (i, (lane, (state, checks))) in lanes.into_iter().zip(states).enumerate() {
            let target = LaneRef { project, lane };
            // One branch per lane, so one answer links each to its own PR.
            let branch = format!("lane-{i}");
            self.seed_push_target_for_shot(target, &branch);
            let summary = pr(480 + i as u64, &branch, "Lane PR", state, checks);
            prs.push(summary.clone());
            self.seed_pr_lookup_for_shot(
                target,
                PrLookup::Linked(Box::new(PrLink {
                    repo: repo(),
                    pr: summary,
                })),
                cx,
            );
        }
        install_fake_gh(GhAuth::Ready, prs, cx);
        PrPoller::seed_for_shot(GITHUB_HOST, GhAvailability::Ready, cx);
        cx.notify();
    }
}
