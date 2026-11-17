//! Resizing the list column beside a detail and hiding or showing it.

use gpui::{Context, Window};

use super::page_layout::{ListColumn, list_width};
use crate::workspace::Workspace;

/// A drag of the divider between the list and the detail, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::workspace) struct DividerDrag {
    anchor: f32,
    start: f32,
}

impl Workspace {
    /// The divider was pressed at `x`; a double press restores the default.
    pub(in crate::workspace) fn press_pr_divider(
        &mut self,
        x: f32,
        click_count: usize,
        cx: &mut Context<Self>,
    ) {
        let page = &mut self.pages.pull_requests;
        if click_count == 2 {
            page.divider_drag = None;
            self.mutate_durable(cx, |ws, _| {
                ws.pages.pull_requests.list_width = crate::ui::theme::PR_SPLIT_LIST_W;
            });
        } else {
            page.divider_drag = Some(DividerDrag {
                anchor: x,
                start: page.list_width(),
            });
        }
        cx.notify();
    }

    /// The pointer moved to `x` during a divider drag: the list follows,
    /// within what the window holds now. Whether a drag was running.
    pub(in crate::workspace) fn drag_pr_divider(&mut self, x: f32, cx: &mut Context<Self>) -> bool {
        let page = &mut self.pages.pull_requests;
        let Some(drag) = page.divider_drag else {
            return false;
        };
        let width = list_width(page.content_width, drag.start + (x - drag.anchor));
        if width != page.list_width {
            page.list_width = width;
            cx.notify();
        }
        true
    }

    /// The release keeps the width the drag reached, for later windows too.
    pub(in crate::workspace) fn end_pr_divider_drag(&mut self, cx: &mut Context<Self>) {
        if self.pages.pull_requests.divider_drag.is_some() {
            self.mutate_durable(cx, |ws, _| ws.pages.pull_requests.divider_drag = None);
            cx.notify();
        }
    }

    /// Show the open detail alone, or the list beside it again. The
    /// keyboard follows what is drawn.
    pub(in crate::workspace) fn toggle_pr_list_column(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mutate_durable(cx, |ws, _| {
            let page = &mut ws.pages.pull_requests;
            page.list_column = match page.list_column {
                ListColumn::Beside => ListColumn::Hidden,
                ListColumn::Hidden => ListColumn::Beside,
            };
        });
        self.focus_shown_page(window, cx);
        cx.notify();
    }

    /// Whether the page offers to hide or show the list beside its detail:
    /// only where the window holds both. `Some(true)` while it is beside.
    pub(in crate::workspace) fn pr_list_column_toggle(&self) -> Option<bool> {
        let page = &self.pages.pull_requests;
        (page.detail.is_some() && page.split_fits())
            .then_some(page.list_column == ListColumn::Beside)
    }

    pub(in crate::workspace) fn pr_divider_dragging(&self) -> bool {
        self.pages.pull_requests.divider_drag.is_some()
    }
}

impl super::PullRequestsPage {
    /// The layout choices a window's saved state keeps.
    pub(in crate::workspace) fn layout_record(&self) -> daruda_store::project::PullRequestsLayout {
        daruda_store::project::PullRequestsLayout {
            list_width: (self.list_width != crate::ui::theme::PR_SPLIT_LIST_W)
                .then_some(self.list_width),
            list_hidden: self.list_column == ListColumn::Hidden,
        }
    }

    /// Take a saved window's layout choices; an unusable width keeps the
    /// default.
    pub(in crate::workspace) fn restore_layout(
        &mut self,
        record: &daruda_store::project::PullRequestsLayout,
    ) {
        self.list_width = record
            .list_width
            .filter(|width| width.is_finite())
            .map_or(crate::ui::theme::PR_SPLIT_LIST_W, |width| {
                width.max(crate::ui::theme::PR_SPLIT_LIST_MIN_W)
            });
        self.list_column = if record.list_hidden {
            ListColumn::Hidden
        } else {
            ListColumn::Beside
        };
    }
}

#[cfg(test)]
mod tests;
