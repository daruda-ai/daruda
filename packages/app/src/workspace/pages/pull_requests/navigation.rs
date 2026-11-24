//! Keyboard browsing of the Pull requests list: the selected row, moving it
//! with the arrows, opening it with Enter, and keeping it in view. The row
//! order is the one last drawn, so the keys follow what the person sees,
//! search narrowing included.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{Bounds, Context, Focusable as _, Pixels, Window, point};

use super::list::{GroupBody, PageBody, PrKey, PrPageView, PrRow};
use crate::workspace::Workspace;
use crate::workspace::pages::Page;

/// Where each drawn row was laid out, recorded by a canvas in the row while
/// painting, so a keyboard move can scroll exactly as far as it needs to.
/// Layout geometry only; refilled each frame the list is drawn.
pub(in crate::workspace) type RowBounds = Rc<RefCell<HashMap<PrKey, Bounds<Pixels>>>>;

/// The rows in the order they are drawn, every group's in turn.
pub(super) fn rows(list: &PrPageView) -> impl Iterator<Item = &PrRow> {
    let groups = match &list.body {
        PageBody::Groups(groups) => groups.as_slice(),
        PageBody::Off | PageBody::Discovering | PageBody::NoRepos => &[],
    };
    groups.iter().flat_map(|group| match &group.body {
        GroupBody::Listed { rows, .. } => rows.as_slice(),
        GroupBody::Loading | GroupBody::Blocked(_) => &[],
    })
}

/// The key of `row`, as the selection holds it.
pub(super) fn key(row: &PrRow) -> PrKey {
    (row.item.repo.clone(), row.item.pr.number)
}

/// The row `delta` steps from `cursor`, stopping at either end. With no
/// selection, or one the shown rows no longer hold, the first step lands on
/// the first row going down and the last going up.
pub(super) fn step(order: &[PrKey], cursor: Option<&PrKey>, delta: isize) -> Option<PrKey> {
    let last = order.len().checked_sub(1)?;
    let at = cursor.and_then(|cursor| order.iter().position(|key| key == cursor));
    let next = match at {
        Some(at) => at.saturating_add_signed(delta).min(last),
        None if delta < 0 => last,
        None => 0,
    };
    Some(order[next].clone())
}

/// The vertical scroll offset that brings `row` fully into `viewport`, or
/// `None` when it is already. Both are where the last frame laid them out,
/// at `offset`; a row taller than the viewport aligns its top.
pub(super) fn reveal(
    viewport: Bounds<Pixels>,
    row: Bounds<Pixels>,
    offset: Pixels,
) -> Option<Pixels> {
    if row.top() < viewport.top() || row.size.height > viewport.size.height {
        Some(offset + (viewport.top() - row.top()))
    } else if row.bottom() > viewport.bottom() {
        Some(offset - (row.bottom() - viewport.bottom()))
    } else {
        None
    }
}

impl Workspace {
    /// Select the shown row `delta` steps away and scroll it into view.
    /// Nothing happens while the list is not drawn.
    pub(in crate::workspace) fn move_pr_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let page = &self.pages.pull_requests;
        let Some(snap) = &page.snapshot else {
            return;
        };
        let order: Vec<PrKey> = rows(&snap.list).map(key).collect();
        let Some(next) = step(&order, page.cursor.as_ref(), delta) else {
            return;
        };
        self.scroll_pr_row_into_view(&next);
        self.pages.pull_requests.cursor = Some(next);
        cx.notify();
    }

    /// Scroll the list so `key`'s row, where the last frame drew it, is in
    /// view. Whether the list moved.
    fn scroll_pr_row_into_view(&self, key: &PrKey) -> bool {
        let page = &self.pages.pull_requests;
        let Some(row) = page.row_bounds.borrow().get(key).copied() else {
            return false;
        };
        let scroll = &self.pages.scrolls[Page::PullRequests.index()];
        let offset = scroll.offset();
        let Some(y) = reveal(scroll.bounds(), row, offset.y) else {
            return false;
        };
        scroll.set_offset(point(offset.x, y));
        true
    }

    /// Bring the selected row back into view after the list was laid out
    /// anew. Deferred from the render that laid it out, so it runs once
    /// that frame has painted and the row bounds are its own.
    pub(in crate::workspace) fn reveal_pr_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.pages.pull_requests.cursor.clone() else {
            return;
        };
        if self.scroll_pr_row_into_view(&cursor) {
            cx.notify();
        }
    }

    /// Open the selected row's detail, as clicking it would.
    pub(in crate::workspace) fn open_pr_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = &self.pages.pull_requests;
        let (Some(snap), Some(cursor)) = (&page.snapshot, &page.cursor) else {
            return;
        };
        let Some(item) = rows(&snap.list)
            .find(|row| key(row) == *cursor)
            .map(|row| row.item.as_ref().clone())
        else {
            return;
        };
        self.open_pr_list_item(item, window, cx);
    }

    /// Open a row's pull request: a first visit starts at the description,
    /// as from anywhere; one visited before keeps its tab, file and scroll.
    pub(super) fn open_pr_list_item(
        &mut self,
        item: daruda_github::PrListItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pr_detail(item, window, cx);
    }

    /// Put the keyboard in the list's search, showing the page's list
    /// first when the page is closed or shows a detail.
    pub(in crate::workspace) fn focus_pr_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_page(Page::PullRequests, cx);
        // Kept whatever the width now: a window narrowed while searching
        // must still show the list over the detail.
        self.pages.pull_requests.searching_list = true;
        self.pages
            .pull_requests
            .search
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    /// Escape leaves the search field before backing out of the detail it came from.
    pub(in crate::workspace) fn leave_pr_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_page() != Some(Page::PullRequests)
            || !(self.pages.pull_requests.searching_list || self.pr_search_focused(window, cx))
        {
            return false;
        }
        self.pages.pull_requests.searching_list = false;
        self.focus_shown_page(window, cx);
        cx.notify();
        true
    }

    pub(in crate::workspace) fn on_pull_requests_select_next(
        &mut self,
        _: &crate::workspace::PullRequestsSelectNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_pr_cursor(1, cx);
    }

    pub(in crate::workspace) fn on_pull_requests_select_prev(
        &mut self,
        _: &crate::workspace::PullRequestsSelectPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_pr_cursor(-1, cx);
    }

    pub(in crate::workspace) fn on_pull_requests_open_selected(
        &mut self,
        _: &crate::workspace::PullRequestsOpenSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_pr_cursor(window, cx);
    }

    pub(in crate::workspace) fn on_pull_requests_focus_search(
        &mut self,
        _: &crate::workspace::PullRequestsFocusSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_pr_search(window, cx);
    }

    /// Select `key`, the pull request whose detail opens, so the list comes
    /// back to it.
    pub(in crate::workspace) fn select_pr_row(&mut self, key: PrKey) {
        self.pages.pull_requests.cursor = Some(key);
    }
}

#[cfg(test)]
impl Workspace {
    pub(in crate::workspace) fn pr_cursor(&self) -> Option<PrKey> {
        self.pages.pull_requests.cursor.clone()
    }
}

impl Workspace {
    pub(in crate::workspace) fn pr_search_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.pages
            .pull_requests
            .search
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
    }
}

#[cfg(test)]
mod tests;
