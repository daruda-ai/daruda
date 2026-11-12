//! Which pull request each lane belongs to. `LaneScoped::pr` changes only
//! through [`replace_pr`], from the three moments that can change it: a new
//! poll answer, a moved push target, and the feature being switched off.

use daruda_github::query::RepoPrs;
use daruda_github::remote_parse::parse_remote;
use daruda_github::{RepoId, link::pick};
use daruda_store::project::LaneRef;
use gpui::Context;

use crate::github::PrLink;
use crate::lane::git::PushTarget;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::LaneScoped;

/// One lane's question for the poller: which PR is `branch` in `repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrDemand {
    pub repo: RepoId,
    pub branch: String,
    /// The lane is the one this window shows, so it is polled more often.
    pub focused: bool,
}

/// Set `state.pr`, reporting whether it changed.
fn replace_pr(state: &mut LaneScoped, link: Option<PrLink>) -> bool {
    if state.pr == link {
        return false;
    }
    state.pr = link;
    true
}

impl Workspace {
    /// Every lane whose push target names a GitHub-shaped repository.
    pub(crate) fn pr_demand(&self) -> Vec<PrDemand> {
        self.lane_scoped
            .iter()
            .filter_map(|(target, state)| {
                let push = state.git.push_target.as_ref()?;
                Some(PrDemand {
                    repo: parse_remote(&push.url)?,
                    branch: push.branch.clone(),
                    focused: *target == self.active,
                })
            })
            .collect()
    }

    /// Apply a poll answer for `repo` to every lane that pushes there. A lane
    /// whose branch the answer does not cover (it moved while the request
    /// ran) keeps its link until the next answer that does.
    pub(crate) fn apply_repo_prs(&mut self, repo: &RepoId, prs: &RepoPrs, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            let Some(push) = state.git.push_target.as_ref() else {
                continue;
            };
            if parse_remote(&push.url).as_ref() != Some(repo) {
                continue;
            }
            let Some(candidates) = prs.by_branch.get(&push.branch) else {
                continue;
            };
            let link = prs
                .default_branch
                .as_deref()
                .and_then(|default| pick(&push.branch, default, candidates))
                .map(|pr| PrLink {
                    repo: repo.clone(),
                    pr: pr.clone(),
                });
            changed |= replace_pr(state, link);
        }
        if changed {
            cx.notify();
        }
    }

    /// Re-read every git lane's push target: the feature was switched on,
    /// and push targets are not kept current while it is off.
    pub(crate) fn refresh_push_targets(&mut self, cx: &mut Context<Self>) {
        let lanes: Vec<LaneRef> = self.lane_scoped.keys().copied().collect();
        for lane in lanes {
            self.refresh_push_target(lane, cx);
        }
    }

    /// Set a lane's link for a capture, through the same write as a poll.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn seed_pr_link_for_shot(
        &mut self,
        lane: LaneRef,
        link: PrLink,
        cx: &mut Context<Self>,
    ) {
        if replace_pr(self.lane_scoped_mut(lane), Some(link)) {
            cx.notify();
        }
    }

    /// Forget every lane's PR: the feature was switched off.
    pub(crate) fn clear_pr_links(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        for state in self.lane_scoped.values_mut() {
            changed |= replace_pr(state, None);
        }
        if changed {
            cx.notify();
        }
    }

    /// Record where `target` pushes. A different repository or branch drops
    /// the old link and asks the poller about the new one at once.
    /// Returns whether anything changed.
    pub(in crate::workspace) fn set_push_target(
        &mut self,
        target: LaneRef,
        found: Option<PushTarget>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(state) = self.lane_scoped.get_mut(&target) else {
            return false;
        };
        if state.git.push_target == found {
            return false;
        }
        let moved = match (&state.git.push_target, &found) {
            (Some(old), Some(new)) => old.url != new.url || old.branch != new.branch,
            _ => true,
        };
        state.git.push_target = found.clone();
        if moved {
            replace_pr(state, None);
            if let Some(repo) = found.as_ref().and_then(|push| parse_remote(&push.url)) {
                crate::github::PrPoller::nudge(repo, cx);
            }
        }
        true
    }
}
