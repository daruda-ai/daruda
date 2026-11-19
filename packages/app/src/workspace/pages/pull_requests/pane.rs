//! A pull request in a worktree's side tabs: the same detail the page
//! shows, read beside the chat that works on it.
//!
//! A worktree has at most one such pane. Opening another pull request
//! there retargets it, as a worktree works on one pull request at a time.
//! The pane holds one of the window's open details (`details.rs`) and lets
//! go of it when it closes or is retargeted.

use daruda_github::PrListItem;
use daruda_store::project::LaneRef;
use gpui::{Context, SharedString, Window};

use super::detail::{PrDetailId, PrOpening};
use crate::workspace::Workspace;
use crate::workspace::main_area::pane::{Pane, PaneContent, PrPaneContent};
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::main_area::side_group::{GroupsShown, SideTab};

/// The tab's label for pull request `number`.
fn label(number: u64) -> SharedString {
    crate::surface::strings::git::pr_number(number).into()
}

impl Workspace {
    /// The active worktree's pull request pane and the detail it holds.
    fn pr_pane(&self) -> Option<(PaneId, PrDetailId)> {
        self.active_runtime()
            .panes
            .iter()
            .find_map(|pane| match &pane.content {
                PaneContent::PullRequest(pr) => Some((pane.id, pr.detail)),
                PaneContent::Terminal(_) | PaneContent::File(_) | PaneContent::AgentChat(_) => None,
            })
    }

    /// The detail a pull request pane shows on screen now, if one does.
    pub(in crate::workspace) fn shown_pr_pane_detail(&self) -> Option<PrDetailId> {
        if self.groups_shown() == GroupsShown::Tabs {
            return None;
        }
        let shown = self.active_runtime().side.shown_pane()?;
        self.pr_pane()
            .filter(|(pane, _)| *pane == shown)
            .map(|(_, detail)| detail)
    }

    /// Show `item` in `lane`'s side tabs and move into it. The pull request
    /// pane the worktree already has is retargeted; the detail is the one
    /// already open for that pull request, when there is one.
    pub(in crate::workspace) fn open_pr_pane(
        &mut self,
        lane: LaneRef,
        item: PrListItem,
        opening: PrOpening,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !crate::github::enabled(cx) {
            return;
        }
        if lane != self.active {
            self.activate_lane(lane, window, cx);
            // Activation re-checks the worktree; one found gone opens nothing.
            if lane != self.active {
                return;
            }
        }
        if self.active_lane_is_inaccessible() {
            return;
        }
        let number = item.pr.number;
        let (id, start) = self.pr_detail_for(item, opening);
        self.pages.pull_requests.details.hold(id);
        let index = match self.pr_pane() {
            Some((pane_id, held)) => {
                if let Some(pane) = self.main_area.pane_mut(pane_id)
                    && let PaneContent::PullRequest(pr) = &mut pane.content
                {
                    pr.detail = id;
                    pr.label = label(number);
                }
                self.release_pr_detail(held, cx);
                self.active_runtime().side.position_of(pane_id)
            }
            None => {
                let pane = Pane {
                    id: self.alloc_id(),
                    content: PaneContent::PullRequest(PrPaneContent {
                        detail: id,
                        label: label(number),
                        focus_handle: cx.focus_handle(),
                    }),
                };
                let tab = SideTab {
                    id: self.alloc_id(),
                    pane: pane.id,
                    user_label: None,
                };
                let runtime = self.active_runtime_mut();
                runtime.panes.push(pane);
                Some(runtime.side.push(tab))
            }
        };
        self.start_pr_detail(id, start, cx);
        self.resize_all_tabs(window, cx);
        if let Some(index) = index {
            self.activate_side_tab(index, window, cx);
        }
        cx.notify();
    }

    /// The active worktree's pull request beside its tabs, a failed
    /// check's log open: the page's sample detail, in a pane.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn open_pr_pane_for_shot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::github_ops::screenshot::PrDetailShot;
        self.seed_pr_detail_for_shot(PrDetailShot::FailedLog, window, cx);
        let Some(item) = self
            .pages
            .pull_requests
            .detail()
            .map(|detail| detail.item().clone())
        else {
            return;
        };
        self.close_page(cx);
        self.open_pr_pane(self.active, item, PrOpening::default(), window, cx);
    }

    /// A pane no longer shows detail `id`. When nothing else does, the
    /// detail ends and leaves its place to come back to.
    pub(in crate::workspace) fn release_pr_detail(&mut self, id: PrDetailId, cx: &mut gpui::App) {
        let page = &mut self.pages.pull_requests;
        if let Some(gone) = page.details.release(id) {
            page.progress.leave(&gone.key(), gone.place());
            // What was typed in it is on disk before it is gone, as when
            // the page's detail closes.
            crate::github::DraftStore::flush(cx);
        }
    }

    /// Panes are going away: the pull request ones let go of their detail.
    pub(in crate::workspace) fn release_pr_panes(&mut self, panes: &[PaneId], cx: &mut gpui::App) {
        let held: Vec<PrDetailId> = panes
            .iter()
            .filter_map(|id| match &self.main_area.pane(*id)?.content {
                PaneContent::PullRequest(pr) => Some(pr.detail),
                PaneContent::Terminal(_) | PaneContent::File(_) | PaneContent::AgentChat(_) => None,
            })
            .collect();
        for id in held {
            self.release_pr_detail(id, cx);
        }
    }

    /// Close every worktree's pull request pane in the window.
    pub(in crate::workspace) fn close_pr_panes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.active;
        let lanes: Vec<LaneRef> = self.main_area.runtimes.keys().copied().collect();
        for lane in lanes {
            let Some(pane) = self.main_area.runtimes.get(&lane).and_then(|runtime| {
                runtime.panes.iter().find_map(|pane| match &pane.content {
                    PaneContent::PullRequest(_) => Some(pane.id),
                    PaneContent::Terminal(_) | PaneContent::File(_) | PaneContent::AgentChat(_) => {
                        None
                    }
                })
            }) else {
                continue;
            };
            if lane == active {
                // The worktree on screen: through the close that settles
                // the keyboard and the widths.
                if let Some(index) = self.active_runtime().side.position_of(pane) {
                    self.close_side_tab(index, window, cx);
                }
                continue;
            }
            self.forget_panes(&[pane], cx);
            if let Some(runtime) = self.main_area.runtimes.get_mut(&lane) {
                if let Some(index) = runtime.side.position_of(pane) {
                    runtime.side.remove(index);
                }
                runtime.panes.retain(|p| p.id != pane);
            }
        }
        cx.notify();
    }

    /// Show on the Pull requests page the pull request `pane` shows.
    pub(in crate::workspace) fn open_pr_pane_on_page(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item = self
            .main_area
            .pane(pane)
            .and_then(|pane| match &pane.content {
                PaneContent::PullRequest(pr) => self
                    .pages
                    .pull_requests
                    .details
                    .get(pr.detail)
                    .map(|detail| detail.item().clone()),
                PaneContent::Terminal(_) | PaneContent::File(_) | PaneContent::AgentChat(_) => None,
            });
        if let Some(item) = item {
            self.show_page(super::Page::PullRequests, cx);
            self.open_pr_detail(item, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    // Covered by the whole-workspace tests in
    // `workspace/tests/pull_request_pane.rs`: a pane needs a window.
}
