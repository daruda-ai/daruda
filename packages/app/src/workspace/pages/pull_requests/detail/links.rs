//! Tying the shown pull request to a worktree by hand: why its worktree is
//! tied to it (its branch name, the user's choice, a fork review mark),
//! taking that away, letting the branch name find it again, and choosing a
//! worktree for it. What the user decides is kept in `LinkStore` per
//! worktree and branch; the lane mirror follows through `sync_lane_links`.

use daruda_github::{PrListItem, RepoId};
use daruda_store::project::LaneRef;
use gpui::Context;

use super::super::list::{LaneFacts, RowLane};
use crate::github::LinkStore;
use crate::github::links::{Source, pr_ref};
use crate::workspace::Workspace;

/// What the detail offers about its worktree link.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::workspace) struct LinkInputs {
    /// Why the linked worktree is tied to it.
    pub source: Option<Source>,
    /// Worktrees pushing to its repository it could be tied to instead.
    pub candidates: Vec<RowLane>,
    /// Worktrees whose branch name finds it but were untied from it.
    pub unlinked: Vec<RowLane>,
    /// Whether what is tied and untied is kept across restarts.
    pub keeping: crate::github::DraftKeeping,
}

impl Workspace {
    /// The link facts for `item`, its worktree `linked` among `lanes`.
    pub(in crate::workspace) fn pr_link_inputs(
        &self,
        item: &PrListItem,
        linked: Option<&LaneFacts>,
        lanes: &[LaneFacts],
        cx: &gpui::App,
    ) -> LinkInputs {
        let key = (item.repo.clone(), item.pr.number);
        let source = linked
            .and_then(|lane| self.lane_scoped.get(&lane.lane))
            .and_then(|state| state.link_source());
        let row = |lane: &LaneFacts| RowLane {
            lane: lane.lane,
            name: lane.name.clone(),
        };
        // A fork review worktree belongs to the pull request it reviews:
        // tying it to another would change nothing.
        let pushes_to = |lane: &LaneFacts| {
            self.lane_scoped
                .get(&lane.lane)
                .and_then(|state| state.github_push())
                .is_some_and(|(repo, push)| repo == item.repo && push.review.is_none())
        };
        let unlinked: Vec<RowLane> = lanes
            .iter()
            .filter(|lane| {
                self.lane_scoped.get(&lane.lane).is_some_and(|state| {
                    state.pr_links.as_ref().is_some_and(|links| {
                        links
                            .suppressed
                            .iter()
                            .any(|pr| crate::github::links::pr_key(pr) == key)
                    })
                })
            })
            .map(row)
            .collect();
        let candidates = if linked.is_some() {
            Vec::new()
        } else {
            lanes
                .iter()
                .filter(|lane| pushes_to(lane))
                .filter(|lane| !unlinked.iter().any(|held| held.lane == lane.lane))
                .map(row)
                .collect()
        };
        LinkInputs {
            source,
            candidates,
            unlinked,
            keeping: LinkStore::keeping(cx),
        }
    }

    /// Tie `lane` to `repo#number` by hand, over what its branch name finds.
    pub(in crate::workspace) fn link_pr_to_lane(
        &mut self,
        lane: LaneRef,
        repo: &RepoId,
        number: u64,
        cx: &mut Context<Self>,
    ) {
        let review = self
            .lane_scoped
            .get(&lane)
            .and_then(|state| state.git.push_target.as_ref())
            .is_some_and(|push| push.review.is_some());
        if review {
            return;
        }
        let Some((path, branch)) = self.lane_worktree_branch(lane) else {
            return;
        };
        let pr = pr_ref(repo, number);
        let now = chrono::Utc::now().timestamp();
        LinkStore::edit(cx, |links| links.choose(&path, &branch, pr, now));
        self.relink_lane(lane, cx);
    }

    /// Untie `lane` from its pull request: a chosen one is dropped, one its
    /// branch name found is not shown again until restored. A fork review
    /// worktree exists for its pull request and is not untied.
    pub(in crate::workspace) fn unlink_pr_lane(&mut self, lane: LaneRef, cx: &mut Context<Self>) {
        let Some(state) = self.lane_scoped.get(&lane) else {
            return;
        };
        // The branch name would find it: then it is kept away from it.
        let named = state.pr.link().is_some_and(|link| {
            !link.pr.cross_repo
                && state
                    .git
                    .push_target
                    .as_ref()
                    .is_some_and(|push| push.branch == link.pr.head_branch)
        });
        let (Some(source), Some((repo, number))) = (
            state.link_source(),
            state.chosen_pr().or_else(|| {
                state
                    .pr
                    .link()
                    .map(|link| (link.repo.clone(), link.pr.number))
            }),
        ) else {
            return;
        };
        if source == Source::Review {
            return;
        }
        let Some((path, branch)) = self.lane_worktree_branch(lane) else {
            return;
        };
        let pr = pr_ref(&repo, number);
        let now = chrono::Utc::now().timestamp();
        LinkStore::edit(cx, |links| {
            links.unlink(
                &path,
                &branch,
                pr,
                source,
                named || source == Source::Branch,
                now,
            )
        });
        self.relink_lane(lane, cx);
    }

    /// The Git panel's Tie again, for the active lane.
    pub(in crate::workspace) fn restore_active_pr_lane(&mut self, cx: &mut Context<Self>) {
        self.restore_pr_lane(self.active, cx);
    }

    /// Let `lane`'s branch name find its pull requests again.
    pub(in crate::workspace) fn restore_pr_lane(&mut self, lane: LaneRef, cx: &mut Context<Self>) {
        let Some((path, branch)) = self.lane_worktree_branch(lane) else {
            return;
        };
        LinkStore::edit(cx, |links| links.restore(&path, &branch));
        self.relink_lane(lane, cx);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn link_inputs_default_offers_nothing() {
        let inputs = super::LinkInputs::default();
        assert!(
            inputs.source.is_none() && inputs.candidates.is_empty() && inputs.unlinked.is_empty()
        );
    }
}
