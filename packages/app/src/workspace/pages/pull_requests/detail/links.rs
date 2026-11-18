//! Tying the shown pull request to a worktree by hand: why its worktree is
//! tied to it (its branch name, the user's choice, a fork review mark),
//! taking that away, letting the branch name find it again, and choosing a
//! worktree for it. What the user decides is kept in `LinkStore` per
//! worktree and branch; the lane mirror follows through `sync_lane_links`.

use crate::workspace::github_ops::link::AfterEdit;
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
    pub keeping: crate::github::Keeping,
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
            self.lane_scoped.get(&lane.lane).is_some_and(|state| {
                state
                    .github_push()
                    .is_some_and(|(repo, _)| repo == item.repo)
                    && !state.lane_link().is_some_and(|link| link.is_review())
            })
        };
        let unlinked: Vec<RowLane> = lanes
            .iter()
            .filter(|lane| {
                self.lane_scoped
                    .get(&lane.lane)
                    .is_some_and(|state| state.suppresses(&key.0, key.1))
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
            .and_then(|state| state.lane_link())
            .is_some_and(|link| link.is_review());
        if review {
            return;
        }
        let pr = pr_ref(repo, number);
        self.edit_lane_links(lane, AfterEdit::Relink, cx, |links, path, branch, now| {
            links.choose(path, branch, pr, now)
        });
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
            state
                .git
                .push_target
                .as_ref()
                .is_some_and(|push| daruda_github::link::heads(&push.branch, &link.pr))
        });
        let (Some(source), Some((repo, number))) = (state.link_source(), state.pr_key()) else {
            return;
        };
        // A review worktree stays with its pull request (`LaneLink::is_review`).
        if source == Source::Review {
            return;
        }
        let pr = pr_ref(&repo, number);
        let keep_away = named || source == Source::Branch;
        self.edit_lane_links(lane, AfterEdit::Relink, cx, |links, path, branch, now| {
            links.unlink(path, branch, pr, source, keep_away, now)
        });
    }

    /// The Git panel's Tie again, for the active lane.
    pub(in crate::workspace) fn restore_active_pr_lane(&mut self, cx: &mut Context<Self>) {
        self.restore_pr_lane(self.active, cx);
    }

    /// Let `lane`'s branch name find its pull requests again.
    pub(in crate::workspace) fn restore_pr_lane(&mut self, lane: LaneRef, cx: &mut Context<Self>) {
        self.edit_lane_links(lane, AfterEdit::Relink, cx, |links, path, branch, _| {
            links.restore(path, branch)
        });
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
