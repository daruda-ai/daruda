//! Task, flow and pull request pages, independent of the right utility dock.

use daruda_store::project::WorkspacePage;
use gpui::{Context, ScrollHandle, Window};

use super::Workspace;
use crate::surface::strings;
use crate::ui::icons;

pub(in crate::workspace) mod detail;
pub(in crate::workspace) mod flows;
pub(in crate::workspace) mod pull_requests;
pub(super) mod render;
pub(in crate::workspace) mod tasks;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum Page {
    Tasks,
    Flows,
    PullRequests,
}

impl Page {
    /// Every page, in navigation order.
    pub const ALL: [Self; 3] = [Self::Tasks, Self::Flows, Self::PullRequests];

    pub fn from_stored(page: WorkspacePage) -> Self {
        match page {
            WorkspacePage::Tasks => Self::Tasks,
            WorkspacePage::Flows => Self::Flows,
        }
    }

    /// What the saved state records; `None` for a page a window does not
    /// reopen on. Pull requests has no stored form: an older build reading
    /// one it does not know would fail to restore the whole window.
    pub fn stored(self) -> Option<WorkspacePage> {
        match self {
            Self::Tasks => Some(WorkspacePage::Tasks),
            Self::Flows => Some(WorkspacePage::Flows),
            Self::PullRequests => None,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Tasks => strings::dock::right_tab_tasks(),
            Self::Flows => strings::dock::right_tab_flows(),
            Self::PullRequests => strings::pull_requests::title(),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Tasks => icons::TASKS,
            Self::Flows => icons::FLOWS,
            Self::PullRequests => icons::PULL_REQUESTS,
        }
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
    pub(in crate::workspace) fn page_state(&self, page: Page) -> PageState {
        let mut state = PageState::new(page);
        if page == Page::Flows {
            state.scroll = self.flows.browser.scrolls[self.flows.browser.state.tab.index()].clone();
        }
        state
    }

    pub(in crate::workspace) fn active_page(&self) -> Option<Page> {
        self.workspace_page.as_ref().map(|state| state.page)
    }

    pub(in crate::workspace) fn show_page(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.active_page() == Some(page) {
            return;
        }
        self.mutate_durable(cx, |ws, _| {
            ws.workspace_page = Some(ws.page_state(page));
            ws.main_area.pending_resize = true;
        });
        self.follow_pr_page(cx);
        if page == Page::Flows {
            self.respawn_flow_watcher(cx);
            if let Some(detail) = &self.pages.flows.detail
                && matches!(detail.body, flows::detail::FlowDetailBody::Run(_))
            {
                self.refresh_execution(detail.id, cx);
            }
        }
        cx.notify();
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

    pub(in crate::workspace) fn close_page(&mut self, cx: &mut Context<Self>) {
        self.pages.flows.history_request = None;
        if self.workspace_page.is_some() {
            self.mutate_durable(cx, |ws, _| {
                ws.workspace_page = None;
                ws.main_area.pending_resize = true;
            });
            self.follow_pr_page(cx);
            cx.notify();
        }
    }

    /// Consume a page close before a command can reach the hidden tab or pane.
    pub(in crate::workspace) fn return_to_worktree(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace_page.is_none() {
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
