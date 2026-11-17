//! Task, flow and pull request pages, independent of the right utility dock.

use daruda_store::project::WorkspacePage;
use gpui::{Context, ScrollHandle, Window};

use super::Workspace;
use super::layout::LeftDockSnapshot;
use crate::surface::strings;
use crate::ui::icons;

pub(in crate::workspace) mod chrome;
pub(in crate::workspace) mod detail;
pub(in crate::workspace) mod flows;
mod location;
pub(in crate::workspace) mod pull_requests;
pub(super) mod render;
pub(in crate::workspace) mod scope;
mod shown;
pub(in crate::workspace) mod tasks;

pub(in crate::workspace) use shown::ShownPage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum Page {
    Tasks,
    Flows,
    PullRequests,
}

/// What navigation and the saved state know of one page. A page's hooks,
/// body and detail are dispatched where they live: `shown.rs`, `render.rs`
/// and `detail.rs`.
struct PageSpec {
    label: fn() -> String,
    icon: &'static str,
    /// What the saved state records; `None` for a page a window does not
    /// reopen on. Pull requests has no stored form: an older build reading
    /// one it does not know would fail to restore the whole window.
    stored: Option<WorkspacePage>,
    /// The count beside the page's navigation entry, for a page that has one.
    count: Option<NavCount>,
}

/// A navigation entry's count and what it counts, which its tooltip says:
/// the number can differ from the page's own tabs, which count otherwise.
#[derive(Clone, Copy)]
struct NavCount {
    of: fn(&LeftDockSnapshot) -> usize,
    means: fn(usize) -> String,
}

impl Page {
    pub const fn index(self) -> usize {
        match self {
            Self::Tasks => 0,
            Self::Flows => 1,
            Self::PullRequests => 2,
        }
    }
    /// Every page, in navigation order.
    pub const ALL: [Self; 3] = [Self::Tasks, Self::Flows, Self::PullRequests];

    const fn spec(self) -> PageSpec {
        match self {
            Self::Tasks => PageSpec {
                label: strings::dock::right_tab_tasks,
                icon: icons::TASKS,
                stored: Some(WorkspacePage::Tasks),
                count: None,
            },
            Self::Flows => PageSpec {
                label: strings::dock::right_tab_flows,
                icon: icons::FLOWS,
                stored: Some(WorkspacePage::Flows),
                count: None,
            },
            Self::PullRequests => PageSpec {
                label: strings::pull_requests::title,
                icon: icons::PULL_REQUESTS,
                stored: None,
                count: Some(NavCount {
                    of: |snap| snap.open_lane_prs,
                    means: |count| strings::pull_requests::nav_count(count),
                }),
            },
        }
    }

    pub fn from_stored(page: WorkspacePage) -> Self {
        match page {
            WorkspacePage::Tasks => Self::Tasks,
            WorkspacePage::Flows => Self::Flows,
        }
    }

    pub fn stored(self) -> Option<WorkspacePage> {
        self.spec().stored
    }

    pub fn label(self) -> String {
        (self.spec().label)()
    }

    pub fn icon(self) -> &'static str {
        self.spec().icon
    }

    /// The count beside the navigation entry and its tooltip; nothing is
    /// shown for 0.
    pub fn nav_count(self, snap: &LeftDockSnapshot) -> Option<(usize, String)> {
        let count = self.spec().count?;
        let n = (count.of)(snap);
        (n > 0).then(|| (n, (count.means)(n)))
    }
}

pub(in crate::workspace) struct PageState {
    pub page: Page,
    pub scroll: ScrollHandle,
}

impl PageState {
    pub fn new(page: Page) -> Self {
        Self {
            page,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Workspace {
    fn page_state(&self, page: Page) -> PageState {
        let mut state = PageState::new(page);
        state.scroll = self.pages.scrolls[page.index()].clone();
        if page == Page::Flows {
            state.scroll = self.flows.browser.scrolls[self.flows.browser.state.tab.index()].clone();
        }
        state
    }

    pub(in crate::workspace) fn active_page(&self) -> Option<Page> {
        self.workspace_page.page()
    }

    pub(in crate::workspace) fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.set_workspace_page(Some(page), cx);
    }

    /// The Flows page came on screen: watch the browsed worktree's flows too,
    /// and catch a shown run up on what it missed while hidden.
    fn flows_page_shown(&mut self, cx: &mut Context<Self>) {
        self.respawn_flow_watcher(cx);
        if let Some(detail) = &self.pages.flows.detail
            && self.page_scope_matches(detail.lane.project)
            && matches!(detail.body, flows::detail::FlowDetailBody::Run(_))
        {
            self.refresh_execution(detail.id, cx);
        }
    }

    pub(in crate::workspace) fn open_page(
        &mut self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_page(page, cx);
        self.window_runtime.focus_handle.focus(window, cx);
    }

    pub(in crate::workspace) fn last_page(&self) -> Option<Page> {
        self.pages.last_page
    }

    pub(in crate::workspace) fn on_resume_workspace_page(
        &mut self,
        _: &super::ResumeWorkspacePage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(page) = self.last_page() {
            self.open_page(page, window, cx);
        }
    }

    pub(in crate::workspace) fn close_page(&mut self, cx: &mut Context<Self>) {
        self.pages.flows.history_request = None;
        self.set_workspace_page(None, cx);
    }

    /// Consume a page close before a command can reach the hidden tab or pane.
    pub(in crate::workspace) fn return_to_worktree(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_page().is_none() {
            return false;
        }
        self.close_page(cx);
        let focused = self.active_runtime().focused_pane_id;
        if self
            .active_runtime()
            .panes
            .iter()
            .any(|pane| pane.id == focused)
        {
            self.focus_pane(focused, window, cx);
        } else {
            self.window_runtime.focus_handle.focus(window, cx);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_page_round_trips() {
        for page in Page::ALL {
            if let Some(stored) = page.stored() {
                assert_eq!(Page::from_stored(stored), page);
            }
        }
        assert_eq!(Page::PullRequests.stored(), None, "never saved");
    }
}
