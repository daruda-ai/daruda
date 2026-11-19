//! The side tab group as drawn: its own tab strip over the pane of the tab
//! shown, and the divider that sets its share of the width.

use gpui::{
    AnyElement, App, Context, CursorStyle, MouseButton, MouseDownEvent, SharedString, div,
    prelude::*, px,
};

use super::menus::{SideTabKind, side_tab_context_menu_items};
use super::tab_bar::{TabCellColors, TabStripColors, tab_strip_colors};
use crate::ui::cursor::CursorReachExt as _;
use crate::ui::theme;
use crate::workspace::main_area::pane::PaneContent;
use crate::workspace::main_area::pane_tree::{DIVIDER_PX, PaneLayout};
use crate::workspace::main_area::render_layout;
use crate::workspace::main_area::side_group::PanePlace;
use crate::workspace::root_menu::RootContextMenuExt as _;
use crate::workspace::{TAB_BAR_HEIGHT, Workspace};

/// One side tab's cell, gathered before the element tree is built so no
/// entity is read while it is.
struct SideCell {
    index: usize,
    label: SharedString,
    title: SharedString,
    is_active: bool,
    /// The tab the next left-dock preview replaces.
    is_scratch: bool,
    kind: SideTabKind,
}

impl Workspace {
    fn side_cells(&self, cx: &App) -> Vec<SideCell> {
        let runtime = self.active_runtime();
        let side = &runtime.side;
        let scratch = match self.preview_slot(cx) {
            Some((PanePlace::Side(index), _)) => Some(index),
            Some((PanePlace::Tab(_), _)) | None => None,
        };
        side.tabs()
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let pane = runtime.panes.iter().find(|pane| pane.id == tab.pane);
                let title = tab
                    .user_label
                    .clone()
                    .or_else(|| pane.map(|pane| pane.title(cx)))
                    .unwrap_or_default();
                let label = if pane.is_some_and(|pane| pane.tab_dirty_dot(cx)) {
                    SharedString::from(format!(
                        "{}{}",
                        crate::surface::glyphs::TAB_TITLE_DIRTY_DOT,
                        title
                    ))
                } else {
                    title.clone()
                };
                SideCell {
                    index,
                    label,
                    title,
                    is_active: index == side.active_index(),
                    is_scratch: scratch == Some(index),
                    kind: match pane.map(|pane| &pane.content) {
                        Some(PaneContent::PullRequest(_)) => SideTabKind::PullRequest(tab.pane),
                        _ => SideTabKind::File,
                    },
                }
            })
            .collect()
    }

    /// The side group: its strip, then the pane of the tab shown.
    pub(super) fn build_side_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let TabStripColors {
            bar_bg,
            bar_border,
            cell: colors,
        } = tab_strip_colors(cx);
        let cells = self.side_cells(cx);
        let count = cells.len();
        // Where the keyboard is reads from the strip: the shown tab of the
        // group without it is drawn as an inactive one.
        let focused = self.keyboard_is_aside();
        let strip = div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(TAB_BAR_HEIGHT))
            .relative()
            .bg(bar_bg)
            .items_center()
            .children(
                cells
                    .into_iter()
                    .map(|cell| self.build_side_cell(cell, count, focused, &colors, cx)),
            )
            .child(
                crate::ui::button_icon("hide-side-tabs", crate::ui::icons::CHEVRON_RIGHT, cx)
                    .tooltip(crate::surface::strings::common::side_tabs_hide())
                    .ml_auto()
                    .mx(px(theme::NEW_TAB_MARGIN_X))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_toggle_side_tabs(&crate::workspace::ToggleSideTabs, window, cx)
                    })),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .bottom_0()
                    .size_full()
                    .border_b_1()
                    .border_color(bar_border),
            );
        let runtime = self.active_runtime();
        // The detail a pull request pane shows, staged for this frame.
        let mut pr_detail = self.shown_pr_pane_detail().and_then(|id| {
            let page = &self.pages.pull_requests;
            let view = page.detail_snapshot.as_ref().filter(|view| view.id == id)?;
            let detail = page.details.get(id)?;
            Some(
                crate::workspace::pages::pull_requests::detail::render::render(
                    view,
                    detail.scroll(),
                    detail.files_scroll(),
                    cx,
                ),
            )
        });
        let content = match runtime.side.active_pane() {
            Some(pane) => render_layout(
                &PaneLayout::Pane(pane),
                &runtime.panes,
                pane,
                false,
                SharedString::from(self.mirrors.font_family.clone()),
                None,
                None,
                None,
                &self.file_send_action(),
                &mut pr_detail,
                cx,
            ),
            None => div().into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .child(strip)
            .child(div().flex_1().min_h_0().w_full().flex().child(content))
            .into_any_element()
    }

    fn build_side_cell(
        &self,
        cell: SideCell,
        count: usize,
        group_focused: bool,
        colors: &TabCellColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let SideCell {
            index,
            label,
            title,
            is_active,
            is_scratch,
            kind,
        } = cell;
        let ws = cx.entity().downgrade();
        let (inactive_bg, inactive_text, hover_bg) = (
            colors.tab_inactive_bg,
            colors.tab_inactive_text,
            colors.tab_inactive_hover_bg,
        );
        let close = crate::ui::button_close(("side-tab-close", index), cx)
            // Closing a tab does not first show it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.mutate_durable_in(window, cx, |ws, window, cx| {
                    ws.request_close_side_tab(index, window, cx)
                });
            }));
        div()
            .id(("side-tab", index))
            .debug_selector(move || format!("side-tab-{index}"))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::TAB_GAP))
            .pl(px(theme::TAB_PAD_LEFT))
            .pr(px(theme::TAB_PAD_RIGHT))
            .py(px(theme::TAB_PAD_Y))
            .mx(px(theme::TAB_MARGIN_X))
            .min_w(px(theme::TAB_MIN_WIDTH))
            .max_w(px(theme::TAB_MAX_WIDTH))
            .flex_grow(1.)
            .flex_basis(px(0.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_size(px(theme::TAB_FONT_SIZE))
            .cursor_pointer()
            .when(is_active, |d| {
                d.bg(colors.tab_active_file_chrome_bg)
                    .text_color(if group_focused {
                        colors.tab_active_text
                    } else {
                        inactive_text
                    })
            })
            .when(!is_active, |d| {
                d.bg(inactive_bg)
                    .text_color(inactive_text)
                    .hover(move |d| d.bg(hover_bg))
            })
            .tooltip(crate::ui::tooltip::text(title))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.mutate_durable_in(window, cx, |ws, window, cx| {
                        ws.activate_side_tab(index, window, cx)
                    });
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, window, cx| {
                    this.mutate_durable_in(window, cx, |ws, window, cx| {
                        ws.request_close_side_tab(index, window, cx)
                    });
                }),
            )
            .root_context_menu(ws.clone(), move |menu, _window, _cx| {
                side_tab_context_menu_items(&ws, index, count, kind)
                    .into_iter()
                    .fold(menu, |menu, item| menu.item(item))
            })
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(is_scratch, |d| d.italic())
                    .child(label),
            )
            .child(close)
    }

    /// The divider between the tab groups: a hairline with a wider strip
    /// over it to take the pointer.
    pub(super) fn build_side_divider(&self, cx: &mut Context<Self>) -> AnyElement {
        let hit = theme::RESIZE_HANDLE_HIT_PX;
        let dragging = self.main_area.side_divider_drag.is_some();
        div()
            .relative()
            .flex_none()
            .w(px(DIVIDER_PX))
            .h_full()
            .bg(theme::current(cx).border)
            .child(
                div()
                    .id("side-tabs-divider")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px((DIVIDER_PX - hit) / 2.0))
                    .w(px(hit))
                    // Hitbox-bound while idle, window-wide once held: the
                    // drag is what takes the pointer off the divider.
                    .cursor_reach(Some(crate::ui::cursor::CursorReach::while_dragging(
                        CursorStyle::ResizeLeftRight,
                        dragging,
                    )))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.begin_side_divider_drag(event.position.x.into(), cx)
                        }),
                    ),
            )
            .into_any_element()
    }
}
