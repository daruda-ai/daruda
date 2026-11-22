//! A pull request in a worktree's side tabs: the same detail the page
//! shows, read beside the chat that works on it.
//!
//! A worktree has at most one such pane. Opening another pull request
//! there retargets it, as a worktree works on one pull request at a time.
//! The pane owns a hold on one of the window's open details
//! (`details.rs`): the detail stays open while the pane does, and is let
//! go of with the pane, however the pane goes.
//!
//! The layout keeps only which pull request the pane shows. Restored, the
//! pane has no detail until that pull request is read again, once its
//! worktree is shown; a failed read is offered again from the pane.

use daruda_github::{GhError, PrDetail, PrListItem, RepoId};
use daruda_store::project::LaneRef;
use gpui::{Context, SharedString, Window};

use super::detail::{PrDetailId, PrOpening};
use super::direct::PrDestination;
use crate::gh_host::GhHost;
use crate::workspace::Workspace;
use crate::workspace::main_area::pane::{
    Pane, PaneContent, PrPaneContent, PrPaneDetail, PrPaneRead,
};
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::main_area::side_group::{GroupsShown, SideTab};

/// The tab's label for pull request `number`.
fn label(number: u64) -> SharedString {
    crate::surface::strings::git::pr_number(number).into()
}

impl Workspace {
    /// The active worktree's pull request pane.
    fn pr_pane(&self) -> Option<PaneId> {
        self.active_runtime()
            .panes
            .iter()
            .find(|pane| pane.content.as_pull_request().is_some())
            .map(|pane| pane.id)
    }

    fn pr_pane_content_mut(&mut self, pane: PaneId) -> Option<&mut PrPaneContent> {
        self.main_area
            .pane_mut(pane)
            .and_then(|pane| pane.content.as_pull_request_mut())
    }

    /// The detail a pull request pane shows on screen now, if one does.
    pub(in crate::workspace) fn shown_pr_pane_detail(&self) -> Option<PrDetailId> {
        if self.groups_shown() == GroupsShown::Tabs {
            return None;
        }
        let shown = self.active_runtime().side.shown_pane()?;
        self.active_runtime()
            .panes
            .iter()
            .find(|pane| pane.id == shown)?
            .content
            .as_pull_request()?
            .detail
            .id()
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
        let (repo, number) = (item.repo.clone(), item.pr.number);
        let (hold, start) = self.pr_detail_for(item, opening, cx);
        let id = hold.id();
        let index = match self.pr_pane() {
            Some(pane_id) => {
                if let Some(pr) = self.pr_pane_content_mut(pane_id) {
                    // The hold it had is dropped: that detail ends unless
                    // the page shows it. A read it was waiting on lands on
                    // nothing.
                    pr.repo = repo;
                    pr.number = number;
                    pr.detail = PrPaneDetail::Open(hold);
                    pr.label = label(number);
                }
                self.active_runtime().side.position_of(pane_id)
            }
            None => {
                let pane = Pane {
                    id: self.alloc_pane_id(),
                    content: PaneContent::PullRequest(PrPaneContent {
                        repo,
                        number,
                        detail: PrPaneDetail::Open(hold),
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
                Some(runtime.side.open(tab))
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

    /// A pull request pane as the layout kept it: `repo#number`, not read
    /// yet.
    pub(in crate::workspace) fn restored_pr_pane(
        &mut self,
        repo: RepoId,
        number: u64,
        cx: &mut Context<Self>,
    ) -> Pane {
        Pane {
            id: self.alloc_pane_id(),
            content: PaneContent::PullRequest(PrPaneContent {
                repo,
                number,
                detail: PrPaneDetail::Unread(PrPaneRead::Waiting),
                label: label(number),
                focus_handle: cx.focus_handle(),
            }),
        }
    }

    /// The worktree on screen now: a pull request pane restored without
    /// its pull request reads it.
    pub(in crate::workspace) fn read_unread_pr_pane(&mut self, cx: &mut Context<Self>) {
        let waiting = self.pr_pane().filter(|pane| {
            self.main_area
                .pane(*pane)
                .and_then(|pane| pane.content.as_pull_request())
                .is_some_and(|pr| matches!(pr.detail, PrPaneDetail::Unread(PrPaneRead::Waiting)))
        });
        if let Some(pane) = waiting {
            self.send_pr_pane_read(pane, cx);
        }
    }

    /// Read again the pull request a restored pane failed to read.
    pub(in crate::workspace) fn retry_pr_pane_read(
        &mut self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) {
        let failed = self
            .main_area
            .pane(pane)
            .and_then(|pane| pane.content.as_pull_request())
            .is_some_and(|pr| matches!(pr.detail, PrPaneDetail::Unread(PrPaneRead::Failed(_))));
        if failed {
            self.send_pr_pane_read(pane, cx);
        }
    }

    fn send_pr_pane_read(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        // Off, nothing asks `gh`; the panes close as it is switched off.
        if !crate::github::enabled(cx) {
            return;
        }
        let Some(pr) = self.pr_pane_content_mut(pane) else {
            return;
        };
        if !matches!(pr.detail, PrPaneDetail::Unread(_)) {
            return;
        }
        let (repo, number) = (pr.repo.clone(), pr.number);
        // The page has it open already: the pane shows that detail, as a
        // pane opened now would, and nothing is read again.
        let details = &self.pages.pull_requests.details;
        if let Some(hold) = details.of(&repo, number).and_then(|id| details.hold(id)) {
            if let Some(pr) = self.pr_pane_content_mut(pane) {
                pr.detail = PrPaneDetail::Open(hold);
            }
            cx.notify();
            return;
        }
        if let Some(pr) = self.pr_pane_content_mut(pane) {
            pr.detail = PrPaneDetail::Unread(PrPaneRead::Reading);
        }
        let gh = GhHost::global(cx);
        let asked = repo.clone();
        crate::workspace::spawn_helpers::spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_detail(&asked, number),
            move |ws, result, cx| ws.land_pr_pane_read(pane, repo, number, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// The read a restored pane sent. It lands only on the pane still
    /// waiting for that pull request: one retargeted or closed since has
    /// nothing to take from it.
    fn land_pr_pane_read(
        &mut self,
        pane: PaneId,
        repo: RepoId,
        number: u64,
        result: Result<PrDetail, GhError>,
        cx: &mut Context<Self>,
    ) {
        let waiting = self
            .main_area
            .pane(pane)
            .and_then(|pane| pane.content.as_pull_request())
            .is_some_and(|pr| {
                pr.repo == repo
                    && pr.number == number
                    && matches!(pr.detail, PrPaneDetail::Unread(PrPaneRead::Reading))
            });
        if !waiting {
            return;
        }
        match result {
            Ok(read) => {
                let item = super::direct::listed(repo, &read);
                let opening = PrOpening {
                    tab: None,
                    read: Some(Box::new(read)),
                };
                let (hold, start) = self.pr_detail_for(item, opening, cx);
                let id = hold.id();
                if let Some(pr) = self.pr_pane_content_mut(pane) {
                    pr.detail = PrPaneDetail::Open(hold);
                }
                self.settle_pr_details(cx);
                self.start_pr_detail(id, start, cx);
            }
            Err(e) => {
                if let Some(pr) = self.pr_pane_content_mut(pane) {
                    pr.detail = PrPaneDetail::Unread(PrPaneRead::Failed(e.to_string()));
                }
                self.report_pr_read(&e, "github.pr_detail.read", cx);
            }
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

    /// The pull request pane above, as a restart leaves it when its read
    /// fails.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn open_unread_pr_pane_for_shot(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pr_pane_for_shot(window, cx);
        if let Some(pr) = self
            .pr_pane()
            .and_then(|pane| self.pr_pane_content_mut(pane))
        {
            pr.detail = PrPaneDetail::Unread(PrPaneRead::Failed(
                "gh: HTTP 502: Bad Gateway (https://api.github.com/graphql)".into(),
            ));
        }
        self.settle_pr_details(cx);
        cx.notify();
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
        let Some(pr) = self
            .main_area
            .pane(pane)
            .and_then(|pane| pane.content.as_pull_request())
        else {
            return;
        };
        let item = pr
            .detail
            .id()
            .and_then(|id| self.pr_detail(id))
            .map(|detail| detail.item().clone());
        match item {
            Some(item) => {
                self.show_page(super::Page::PullRequests, cx);
                self.open_pr_detail(item, window, cx);
            }
            // Not read yet: the page reads it.
            None => {
                let (repo, number) = (pr.repo.clone(), pr.number);
                self.open_pr_at(repo, number, PrDestination::Page, window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Covered by the whole-workspace tests in
    // `workspace/tests/pull_request_pane.rs`: a pane needs a window.
}
