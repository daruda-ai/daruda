//! Which page is on screen, and the one way to change it. Every change runs
//! the pages' shown/hidden hooks, so what a page starts or stops while it is
//! on screen (the Pull requests search) cannot drift from what is drawn.

use gpui::{Context, ScrollHandle};

use super::{Page, PageState};
use crate::workspace::Workspace;

/// The page shown in place of the lane's tabs. Written only through
/// [`Workspace::set_workspace_page`] and [`Workspace::restore_workspace_page`].
#[derive(Default)]
pub(in crate::workspace) struct ShownPage(Option<PageState>);

impl ShownPage {
    pub fn state(&self) -> Option<&PageState> {
        self.0.as_ref()
    }

    pub fn page(&self) -> Option<Page> {
        self.0.as_ref().map(|state| state.page)
    }

    /// Scroll `page` with `scroll` from now on, if it is the one shown.
    pub fn set_scroll(&mut self, page: Page, scroll: ScrollHandle) {
        if let Some(state) = self.0.as_mut().filter(|state| state.page == page) {
            state.scroll = scroll;
        }
    }
}

impl Workspace {
    /// Show `page`, or the lane's tabs for `None`. The one write of the shown
    /// page after restore; it is saved, and the pages' hooks follow it.
    pub(in crate::workspace) fn set_workspace_page(
        &mut self,
        page: Option<Page>,
        cx: &mut Context<Self>,
    ) {
        let old = self.active_page();
        if old == page {
            return;
        }
        self.mutate_durable(cx, |ws, _| {
            ws.workspace_page.0 = page.map(|page| ws.page_state(page));
            ws.main_area.pending_resize = true;
        });
        self.page_visibility_changed(old, page, cx);
        cx.notify();
    }

    /// Adopt the saved page. Not saved again: it is what was saved.
    pub(in crate::workspace) fn restore_workspace_page(
        &mut self,
        page: Option<Page>,
        cx: &mut Context<Self>,
    ) {
        let old = self.active_page();
        self.workspace_page.0 = page.map(|page| self.page_state(page));
        if old != page {
            self.page_visibility_changed(old, page, cx);
        }
    }

    fn page_visibility_changed(
        &mut self,
        old: Option<Page>,
        new: Option<Page>,
        cx: &mut Context<Self>,
    ) {
        if let Some(old) = old {
            self.on_page_hidden(old, cx);
        }
        if let Some(new) = new {
            self.on_page_shown(new, cx);
        }
    }

    /// What `page` starts when it comes on screen.
    fn on_page_shown(&mut self, page: Page, cx: &mut Context<Self>) {
        match page {
            Page::Tasks => {}
            Page::Flows => self.flows_page_shown(cx),
            Page::PullRequests => self.pr_page_shown(cx),
        }
    }

    /// What `page` stops when it leaves the screen.
    fn on_page_hidden(&mut self, page: Page, cx: &mut Context<Self>) {
        match page {
            Page::Tasks | Page::Flows => {}
            Page::PullRequests => self.hide_pr_list(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, px};

    use super::*;

    #[test]
    fn a_scroll_reaches_only_the_page_shown() {
        let moved = ScrollHandle::new();
        moved.set_offset(point(px(0.), px(-40.)));
        // (label, page scrolled, the shown page moves)
        let cases = [
            ("the shown page", Page::Flows, true),
            ("another page", Page::Tasks, false),
        ];
        for (label, page, moves) in cases {
            let mut shown = ShownPage(Some(PageState::new(Page::Flows)));
            shown.set_scroll(page, moved.clone());
            let offset = shown.state().expect("shown").scroll.offset();
            assert_eq!(offset == moved.offset(), moves, "{label}");
        }
        let mut hidden = ShownPage::default();
        hidden.set_scroll(Page::Flows, moved);
        assert_eq!(hidden.page(), None, "a hidden page stays hidden");
    }
}
