//! The handoff panel: what goes to an agent, the chat it goes to, and the
//! prompt itself before it is inserted.

use gpui::{Anchor, AnyElement, Context, div, prelude::*, px};

use super::PrDetailId;
use super::handoff::{Payload, Prepared};
use super::render::note;
use super::view::{HandoffBlock, HandoffView};
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{
    Disableable as _, DropdownMenu as _, PopupMenuItem, button_compact, button_primary,
    button_with_icon, icons, menu_builder, theme,
};
use crate::workspace::Workspace;

pub(super) fn handoff(id: PrDetailId, view: &HandoffView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let what = match &view.payload {
        Payload::FailedJob(_) => s::handoff_check_log(),
        Payload::FailedChecks => s::handoff_failed_checks(),
        Payload::OpenThreads => s::handoff_open_threads(),
        Payload::SelectedCode(_) => s::handoff_selected_code(),
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
    let ready = ready
        && view
            .editor
            .as_ref()
            .is_none_or(|editor| !editor.read(cx).value().trim().is_empty());
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
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(theme::GAP_SM))
                .children(
                    (!view.chats.is_empty() && !view.checkout).then(|| chat_picker(id, view, cx)),
                )
                .child(if view.checkout {
                    button_primary("pr-handoff-checkout", s::handoff_create_lane())
                        .loading(view.creating)
                        .disabled(!ready || view.creating || view.blocked.is_some())
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.checkout_for_pr_handoff(id, window, cx)
                        }))
                } else if view.chats.is_empty() {
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
                .when(ready && view.editor.is_none(), |row| {
                    row.child(
                        button_compact("pr-handoff-edit", s::handoff_edit())
                            .debug_selector(|| "pr-handoff-edit".into())
                            .on_click(cx.listener(move |ws, _, window, cx| {
                                ws.edit_pr_handoff(id, window, cx)
                            })),
                    )
                })
                .child(
                    button_compact("pr-handoff-close", common::btn_cancel())
                        .on_click(cx.listener(move |ws, _, _, cx| ws.close_pr_handoff(id, cx))),
                ),
        )
        .children(if view.checkout {
            Some(note(s::handoff_branch(&view.lane), cx))
        } else {
            view.chats
                .is_empty()
                .then(|| note(s::handoff_no_chat(&view.lane), cx))
        })
        .child(match &view.editor {
            Some(editor) => crate::ui::markdown_editor(editor, cx).into_any_element(),
            None => preview,
        })
        .into_any_element()
}

fn chat_picker(id: PrDetailId, view: &HandoffView, cx: &Context<Workspace>) -> AnyElement {
    let chosen = view.chosen;
    let label = |chat: &crate::workspace::github_ops::review_send::WritableChat| {
        s::handoff_chat_option(&chat.title, s::handoff_tab(chat.tab))
    };
    let current = view.chats.iter().find(|chat| Some(chat.pane) == chosen);
    let name = current.map_or_else(s::handoff_choose_chat, label);
    let entries: Vec<_> = view
        .chats
        .iter()
        .map(|chat| (chat.pane, label(chat)))
        .collect();
    let workspace = cx.weak_entity();
    button_with_icon("pr-handoff-chat", name, icons::EXPAND_MORE)
        .max_w_full()
        .tooltip(s::handoff_choose_chat())
        .dropdown_menu_with_anchor(
            Anchor::TopLeft,
            menu_builder(move |menu, _, _| {
                entries.iter().fold(menu, |menu, (pane, name)| {
                    let pane = *pane;
                    let workspace = workspace.clone();
                    menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(chosen == Some(pane))
                            .on_click(move |_, _, cx| {
                                if let Some(ws) = workspace.upgrade() {
                                    ws.update(cx, |ws, cx| ws.choose_pr_handoff_chat(id, pane, cx));
                                }
                            }),
                    )
                })
            }),
        )
        .into_any_element()
}
