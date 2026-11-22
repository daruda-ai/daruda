//! What a restored pull request pane draws before it has its detail: the
//! read under way, or why it failed with Try again.

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, div, px,
};

use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{button_compact, theme};
use crate::workspace::Workspace;
use crate::workspace::main_area::pane::PrPaneRead;
use crate::workspace::main_area::pane_tree::PaneId;

/// The body of pull request pane `pane`, its pull request not read yet.
pub(in crate::workspace) fn unread(
    pane: PaneId,
    read: &PrPaneRead,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let (bg, text) = {
        let t = theme::current(cx);
        (t.dock_bg, t.text_subtle)
    };
    let note = div()
        .p(px(theme::DOCK_PAGE_PAD))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_LG))
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(text);
    let note = match read {
        PrPaneRead::Waiting | PrPaneRead::Reading => note.child(common::loading()),
        PrPaneRead::Failed(reason) => {
            let selector = format!("pr-pane-retry-{}", pane.0);
            note.child(div().min_w_0().child(s::detail_failed(reason)))
                .child(
                    button_compact(SharedString::from(selector.clone()), s::retry())
                        .debug_selector(move || selector.clone())
                        .on_click(cx.listener(move |ws, _, _, cx| ws.retry_pr_pane_read(pane, cx))),
                )
        }
    };
    // Filled as the detail it stands in for is.
    div().size_full().bg(bg).child(note).into_any_element()
}

#[cfg(test)]
mod tests {
    // Drawn in `workspace/tests/pull_request_pane.rs`, which reads a
    // restored pane's note: it needs a window.
}
