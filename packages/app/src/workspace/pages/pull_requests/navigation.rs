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
        let row = page.row_bounds.borrow().get(&next).copied();
        let scroll = &self.pages.scrolls[Page::PullRequests.index()];
        if let Some(row) = row {
            let offset = scroll.offset();
            if let Some(y) = reveal(scroll.bounds(), row, offset.y) {
                scroll.set_offset(point(offset.x, y));
            }
        }
        self.pages.pull_requests.cursor = Some(next);
        cx.notify();
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
        self.leave_page_detail_then(Page::PullRequests, window, cx, |ws, window, cx| {
            let search = ws.pages.pull_requests.search.read(cx).focus_handle(cx);
            search.focus(window, cx);
        });
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
