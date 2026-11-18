//! The handoff panel: what goes to an agent, the chat it goes to, and the
//! prompt itself before it is inserted.

use gpui::{AnyElement, Context, div, prelude::*, px};

use super::PrDetailId;
use super::handoff::{Payload, Prepared};
use super::render::note;
use super::view::{HandoffBlock, HandoffView};
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{Disableable as _, button_compact, button_primary, icons, theme};
use crate::workspace::Workspace;

pub(super) fn handoff(id: PrDetailId, view: &HandoffView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let what = match &view.payload {
        Payload::FailedChecks => s::handoff_failed_checks(),
        Payload::OpenThreads => s::handoff_open_threads(),
        Payload::SelectedCode(_) => s::handoff_selected_code(),
    };
    let chats: AnyElement = if view.chats.is_empty() {
        note(s::handoff_no_chat(&view.lane), cx).into_any_element()
    } else {
        // One row per chat, with its tab: chats often share a title.
        div()
            .flex()
            .flex_col()
            .gap(px(theme::GAP_XS))
            .children(
                view.chats
                    .iter()
                    .enumerate()
                    .map(|(index, (pane, title, tab))| {
                        let (pane, chosen) = (*pane, view.chosen == Some(*pane));
                        div()
                            .id(("pr-handoff-chat", index))
                            .debug_selector(move || format!("pr-handoff-chat-{index}"))
                            .flex()
                            .items_center()
                            .gap(px(theme::GAP_SM))
                            .px(px(theme::PAD_SM))
                            .py(px(theme::PAD_XS))
                            .rounded(px(theme::RADIUS_SM))
                            .cursor_pointer()
                            .text_size(px(theme::FONT_SIZE_SM))
                            .when(chosen, |row| row.bg(t.lane_card_active_bg))
                            .when(!chosen, |row| row.hover(|style| style.bg(t.overlay_hover)))
                            .child(
                                icons::icon(if chosen {
                                    icons::RADIO_ON
                                } else {
                                    icons::RADIO_OFF
                                })
                                .text_color(t.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(t.text_body)
                                    .child(title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(t.text_subtle)
                                    .child(s::handoff_tab(*tab)),
                            )
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.choose_pr_handoff_chat(id, pane, cx)
                            }))
                    }),
            )
            .into_any_element()
    };
    let (preview, ready) = match &view.prompt {
        Prepared::Reading(_) => (note(common::loading(), cx).into_any_element(), false),
        Prepared::Nothing => (note(s::handoff_nothing(), cx).into_any_element(), false),
        Prepared::Failed(reason) => (
            note(s::handoff_failed(reason), cx).into_any_element(),
            false,
        ),
        Prepared::Ready(prompt) => (
            div()
                .id("pr-handoff-preview")
                .debug_selector(|| "pr-handoff-preview".into())
                .max_h(px(theme::PR_HANDOFF_PREVIEW_MAX_H))
                .overflow_y_scroll()
                .p(px(theme::PAD_SM))
                .rounded(px(theme::RADIUS_SM))
                .bg(t.overlay_hover)
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_body)
                .whitespace_normal()
                .child(prompt.clone())
                .into_any_element(),
            true,
        ),
    };
    div()
        .debug_selector(|| "pr-handoff".into())
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .p(px(theme::PAD_LG))
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(t.border)
        .child(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(s::handoff_heading(what, &view.lane)),
        )
        .child(chats)
        .children(view.blocked.map(|block| {
            note(
                match block {
                    HandoffBlock::Moved => s::handoff_lane_moved(),
                    HandoffBlock::SelectionChanged => s::handoff_selection_changed(),
                    HandoffBlock::ChatGone => s::handoff_chat_gone(),
                },
                cx,
            )
        }))
        .child(preview)
        .child(
            div()
                .flex()
                .gap(px(theme::GAP_SM))
                .child(if view.chats.is_empty() {
                    // No chat there: one is opened to take it.
                    button_primary("pr-handoff-new-chat", s::handoff_new_chat())
                        .debug_selector(|| "pr-handoff-new-chat".into())
                        .disabled(!ready || view.blocked.is_some())
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.insert_pr_handoff_in_new_chat(id, window, cx)
                        }))
                } else {
                    button_primary("pr-handoff-insert", s::handoff_insert())
                        .debug_selector(|| "pr-handoff-insert".into())
                        .disabled(!ready || view.chosen.is_none() || view.blocked.is_some())
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.insert_pr_handoff(id, window, cx)
                        }))
                })
                .child(
                    button_compact("pr-handoff-close", common::btn_cancel())
                        .on_click(cx.listener(move |ws, _, _, cx| ws.close_pr_handoff(id, cx))),
                ),
        )
        .into_any_element()
}
