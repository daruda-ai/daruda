//! Page navigation and the central page shell reuse the staged dock data.

use gpui::{AnyElement, App, Context, IntoElement, Window, div, prelude::*, px};

use super::{Page, PageState};
use crate::ui::scrollbar::{Scrollbar, ScrollbarMode};
use crate::ui::{ButtonVariants as _, button_with_icon_sized, theme};
use crate::workspace::Workspace;
use crate::workspace::layout::{DockSnapshot, LeftDockSnapshot, RightDockSnapshot};
use crate::workspace::right_dock;

pub(in crate::workspace) fn navigation(snap: &LeftDockSnapshot, cx: &App) -> AnyElement {
    let t = theme::current(cx);
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(theme::DOCK_NAV_ROW_GAP))
        .px(px(theme::DOCK_NAV_PAD_X))
        .py(px(theme::DOCK_NAV_PAD_Y))
        .border_b_1()
        .border_color(t.border)
        .children(Page::ALL.into_iter().map(|page| {
            let workspace = snap.workspace.clone();
            let count = page.nav_count(snap);
            button_with_icon_sized(page.icon(), page.label(), page.icon(), snap.metrics)
                .ghost()
                .w_full()
                .justify_start()
                .h(px(snap.metrics.target(theme::DOCK_NAV_ROW_HEIGHT)))
                .text_size(px(snap.metrics.text(theme::LANE_LABEL_FONT_SIZE)))
                .when(snap.workspace_page == Some(page), |button| {
                    button.bg(t.lane_card_active_bg).text_color(t.text_primary)
                })
                .when_some(count, |button, (count, means)| {
                    button.tooltip(means).child(
                        div()
                            .ml_auto()
                            .text_size(px(snap.metrics.text(theme::FONT_SIZE_XS)))
                            .text_color(t.text_subtle)
                            .child(count.to_string()),
                    )
                })
                .on_click(move |_, window, cx| {
                    if let Some(ws) = workspace.upgrade() {
                        ws.update(cx, |ws, cx| ws.open_page(page, window, cx));
                    }
                })
        }))
        .into_any_element()
}

/// What the shown page draws in the center: its detail when it holds one,
/// its list otherwise. `None` until the list is first staged, so the caller
/// draws what it would without a page. The detail is read off the
/// workspace, not a dock's snapshot.
pub(in crate::workspace) fn body(
    ws: &Workspace,
    state: &PageState,
    window: &Window,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    match state.page {
        Page::Tasks => {
            if let Some(detail) = &ws.pages.tasks.detail
                && ws.shown_page_detail_id().is_some()
            {
                return Some(detail_frame(
                    ws,
                    state.page,
                    super::tasks::editor::render_detail(detail.id, &detail.editor, cx),
                    cx,
                ));
            }
            let DockSnapshot::Right(snap) = &ws.docks.right.read(cx).snap else {
                return None;
            };
            Some(content(state, snap, cx))
        }
        Page::Flows => {
            if let Some(detail) = &ws.pages.flows.detail
                && ws.shown_page_detail_id().is_some()
            {
                return Some(detail_frame(
                    ws,
                    state.page,
                    super::flows::detail_render::render(
                        detail,
                        state,
                        ws.flow_session_problem(detail.id),
                        ws.execution_layout(window, cx),
                        cx,
                    ),
                    cx,
                ));
            }
            let snap = ws.flows.browser.page_snapshot.as_ref()?;
            Some(flow_content(state, snap, cx))
        }
        Page::PullRequests => {
            use super::pull_requests::page_layout::PageLayout;
            use super::pull_requests::row_layout::RowLayout;
            let pages = &ws.pages.pull_requests;
            let layout = ws.pr_page_layout();
            let detail = |cx: &mut Context<Workspace>| -> Option<AnyElement> {
                let view = pages.detail_snapshot.as_ref()?;
                let scroll = pages.detail.as_ref()?.scroll();
                Some(super::pull_requests::detail::render::render(
                    view, scroll, cx,
                ))
            };
            if layout == PageLayout::Detail {
                let body = detail(cx)?;
                return Some(detail_frame(ws, state.page, body, cx));
            }
            let focused = pages.focus_handle().is_focused(window);
            let snap = pages.snapshot.as_ref()?;
            if layout == PageLayout::List {
                let rows = RowLayout::for_width(ws.page_content_width(window, cx));
                return Some(pr_content(state, snap, rows, focused, cx));
            }
            let list_w = pages.list_width();
            let list = pr_content(state, snap, RowLayout::for_width(list_w), focused, cx);
            let detail = detail(cx)?;
            Some(pr_split(ws, state.page, list_w, list, detail, cx))
        }
    }
}

/// The list column beside the detail: the list keeps its header, search
/// and scroll; the detail keeps its location bar with Back and its own
/// scroll.
fn pr_split(
    ws: &Workspace,
    page: Page,
    list_w: f32,
    list: AnyElement,
    detail: AnyElement,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    use crate::ui::cursor::{CursorReach, CursorReachExt as _};
    let t = theme::current(cx);
    let hit = theme::RESIZE_HANDLE_HIT_PX;
    // Not `.cursor()`: a hitbox-bound style stops applying the moment the drag
    // pulls the pointer off these few pixels.
    let reach =
        CursorReach::while_dragging(gpui::CursorStyle::ResizeLeftRight, ws.pr_divider_dragging());
    div()
        .relative()
        .flex()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(t.dock_bg)
        .child(
            div()
                .debug_selector(|| "pr-split-list".into())
                .flex_none()
                .w(px(list_w))
                .h_full()
                .min_h_0()
                .border_r_1()
                .border_color(t.border)
                .child(list),
        )
        .child(
            div()
                .debug_selector(|| "pr-split-detail".into())
                .flex_1()
                .min_w_0()
                .h_full()
                .min_h_0()
                .flex()
                .flex_col()
                .children(super::location::render(ws, page, cx))
                .child(div().flex_1().min_h_0().child(detail)),
        )
        .child(
            div()
                .id("pr-split-divider")
                .debug_selector(|| "pr-split-divider".into())
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(list_w - hit / 2.0))
                .w(px(hit))
                .cursor_reach(Some(reach))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|ws, ev: &gpui::MouseDownEvent, _, cx| {
                        ws.press_pr_divider(ev.position.x.into(), ev.click_count, cx)
                    }),
                ),
        )
        .into_any_element()
}

fn detail_frame(
    ws: &Workspace,
    page: Page,
    body: AnyElement,
    cx: &Context<Workspace>,
) -> AnyElement {
    let header = crate::ui::list_page::header(
        page.label(),
        super::chrome::project_picker(&ws.scope_snapshot(), cx.weak_entity(), cx),
        div(),
        super::chrome::return_button(cx.weak_entity()),
        cx,
    );
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(theme::current(cx).dock_bg)
        .child(
            div()
                .flex_none()
                .border_b_1()
                .border_color(theme::current(cx).border)
                .child(header),
        )
        .children(super::location::render(ws, page, cx))
        .child(div().flex_1().min_h_0().child(body))
        .into_any_element()
}

pub(in crate::workspace) fn content(
    state: &PageState,
    snap: &RightDockSnapshot,
    cx: &App,
) -> AnyElement {
    let close = close_button((*snap.workspace).clone(), cx);
    frame(
        state,
        right_dock::tasks::header(snap, close, cx),
        right_dock::tasks::render(snap, cx),
        cx,
    )
}

pub(in crate::workspace) fn flow_content(
    state: &PageState,
    snap: &crate::workspace::flow_browser::FlowPageSnapshot,
    cx: &App,
) -> AnyElement {
    let close = close_button(snap.workspace.clone(), cx);
    frame(
        state,
        super::flows::header(snap, close, cx),
        super::flows::render(snap, cx),
        cx,
    )
}

pub(in crate::workspace) fn pr_content(
    state: &PageState,
    snap: &super::pull_requests::PrPageSnapshot,
    rows: super::pull_requests::row_layout::RowLayout,
    focused: bool,
    cx: &App,
) -> AnyElement {
    let close = close_button(snap.workspace.clone(), cx);
    frame(
        state,
        super::pull_requests::render::header(snap, rows, close, cx),
        super::pull_requests::render::render(snap, rows, focused, cx),
        cx,
    )
}

fn close_button(workspace: gpui::WeakEntity<Workspace>, _cx: &App) -> AnyElement {
    super::chrome::return_button(workspace)
}

pub(in crate::workspace) fn frame(
    state: &PageState,
    header: AnyElement,
    body: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = theme::current(cx);
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(t.dock_bg)
        .child(
            div()
                .flex_none()
                .border_b_1()
                .border_color(t.border)
                .child(header),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("workspace-page-scroll")
                        .debug_selector(|| "workspace-page-scroll".into())
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .restrict_scroll_to_axis()
                        .track_scroll(&state.scroll)
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .debug_selector(|| "workspace-page-content".into())
                                .p(px(theme::DOCK_PAGE_PAD))
                                .child(body),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .debug_selector(|| "workspace-page-scrollbar".into())
                        .child(
                            Scrollbar::vertical(&state.scroll)
                                .id("workspace-page-scrollbar")
                                .mode(ScrollbarMode::Always),
                        ),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
