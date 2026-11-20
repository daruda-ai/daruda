//! A pull request in a worktree's side tabs: the same detail the page
//! shows, read beside the chat that works on it.
//!
//! A worktree has at most one such pane. Opening another pull request
//! there retargets it, as a worktree works on one pull request at a time.
//! The pane owns a hold on one of the window's open details
//! (`details.rs`): the detail stays open while the pane does, and is let
//! go of with the pane, however the pane goes.

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
            .find_map(|pane| Some((pane.id, pane.content.as_pull_request()?.detail.id())))
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
        let (hold, start) = self.pr_detail_for(item, opening, cx);
        let id = hold.id();
        let index = match self.pr_pane() {
            Some((pane_id, _)) => {
                if let Some(pr) = self
                    .main_area
                    .pane_mut(pane_id)
                    .and_then(|pane| pane.content.as_pull_request_mut())
                {
                    // The hold it had is dropped: that detail ends unless
                    // the page shows it.
                    pr.detail = hold;
                    pr.label = label(number);
                }
                self.active_runtime().side.position_of(pane_id)
            }
            None => {
                let pane = Pane {
                    id: self.alloc_pane_id(),
                    content: PaneContent::PullRequest(PrPaneContent {
                        detail: hold,
                        label: label(number),
                        focus_handle: cx.focus_handle(),
                    }),
                };
                let tab = SideTab {
                    id: self.alloc_tab_id(),
                    pane: pane.id,
                    user_label: None,
                };
                let runtime = self.active_runtime_mut();
                runtime.panes.push(pane);
                Some(runtime.side.push(tab))
            }
        };
        self.settle_pr_details(cx);
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

    /// Panes are going away. A pull request pane drops its hold with
    /// itself; once it has, what its detail leaves behind is settled.
    pub(in crate::workspace) fn settle_pr_details_after(
        &mut self,
        panes: &[PaneId],
        cx: &mut Context<Self>,
    ) {
        let any = panes.iter().any(|id| {
            self.main_area
                .pane(*id)
                .is_some_and(|pane| pane.content.as_pull_request().is_some())
        });
        if !any {
            return;
        }
        let workspace = cx.weak_entity();
        // Deferred: the panes are still in their runtime here.
        cx.defer(move |cx| {
            // SILENT-OK: a workspace gone with its panes has nothing to settle.
            let _ = workspace.update(cx, |ws, cx| ws.settle_pr_details(cx));
        });
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
                runtime
                    .panes
                    .iter()
                    .find(|pane| pane.content.as_pull_request().is_some())
                    .map(|pane| pane.id)
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
            .and_then(|pane| pane.content.as_pull_request())
            .and_then(|pr| self.pr_detail(pr.detail.id()))
            .map(|detail| detail.item().clone());
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
