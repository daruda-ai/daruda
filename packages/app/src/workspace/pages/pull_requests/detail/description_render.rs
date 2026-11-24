//! The Description tab: the description, or its editor for a viewer GitHub
//! lets edit it, and under it the conversation on the pull request.

use gpui::{AnyElement, Context, Entity, div, prelude::*, px};

use super::PrDetailId;
use super::description::SaveState;
use super::render::note;
use super::view::{CommentView, ConversationView, DiscussionView, ReadyView};
use crate::surface::strings::{common, git as git_s, pull_requests as s};
use crate::ui::{
    Disableable as _, TextareaState, button_compact, button_icon, icons, markdown_editor, theme,
};
use crate::workspace::Workspace;

pub(super) fn description(
    id: PrDetailId,
    ready: &ReadyView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let Some(discussion) = ready.discussion.as_ref() else {
        return div().into_any_element();
    };
    let body = match &discussion.edit {
        Some((input, state)) => editor(
            id,
            input,
            state,
            ready.description.as_deref(),
            discussion.keeping,
            cx,
        ),
        None => shown_body(id, ready.description.as_deref(), discussion, cx),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(theme::PAD_XL))
        .child(body)
        .when(discussion.edit.is_none(), |el| {
            el.child(div().flex().items_start().child(
                button_compact("pr-description-files", s::view_changed_files()).on_click(
                    cx.listener(move |ws, _, _, cx| {
                        ws.select_pr_detail_tab(id, super::PrTab::Files, cx)
                    }),
                ),
            ))
        })
        .child(conversation(id, &discussion.conversation, cx))
        .into_any_element()
}

fn shown_body(
    id: PrDetailId,
    body: Option<&str>,
    discussion: &DiscussionView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let text = match body {
        Some(body) => {
            crate::ui::markdown(("pr-detail-description", id.0 as usize), body.to_owned())
                .selectable(true)
                .full_width(true)
                .link_click_handler(super::open_description_link)
                .into_any_element()
        }
        None => note(s::description_empty(), cx).into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .when(discussion.can_edit, |column| {
            column.child(
                div().flex().justify_end().child(
                    button_compact(
                        "pr-body-edit",
                        if discussion.draft {
                            s::body_continue_draft()
                        } else {
                            s::body_edit()
                        },
                    )
                    .debug_selector(|| "pr-body-edit".into())
                    .on_click(
                        cx.listener(move |ws, _, window, cx| ws.edit_pr_body(id, window, cx)),
                    ),
                ),
            )
        })
        .child(text)
        .into_any_element()
}

/// The description being edited, with Save and Cancel and how saving went.
fn editor(
    id: PrDetailId,
    input: &Entity<TextareaState>,
    state: &SaveState,
    remote: Option<&str>,
    keeping: crate::github::DraftKeeping,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let saving = *state == SaveState::Saving;
    let conflict = *state == SaveState::Conflict;
    let line = match state {
        SaveState::Editing => None,
        SaveState::Conflict => Some((s::body_conflict(), t.banner_warning_text)),
        SaveState::Saving => Some((s::body_saving(), t.text_muted)),
        SaveState::Failed(reason) => Some((
            s::body_save_failed_line(s::body_save_failed(), reason),
            t.banner_error_text,
        )),
    };
    div()
        .debug_selector(|| "pr-body-editor".into())
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        // Read-only while it saves: text typed then would be dropped with
        // the editor when the save lands.
        .when(conflict, |el| {
            el.child(note(s::body_remote_version(), cx))
                .child(
                    div().text_color(t.text_body).child(
                        remote
                            .map(str::to_owned)
                            .unwrap_or_else(s::description_empty),
                    ),
                )
                .child(note(s::body_your_draft(), cx))
        })
        .child(markdown_editor(input, cx).disabled(saving))
        .children(super::render::keeping_note(keeping, cx))
        .children(line.map(|(text, color)| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(color)
                .child(text)
        }))
        .child(
            div()
                .flex()
                .gap(px(theme::GAP_SM))
                .child(
                    button_compact("pr-body-save", s::body_save())
                        .debug_selector(|| "pr-body-save".into())
                        .disabled(saving || conflict)
                        .on_click(cx.listener(move |ws, _, _, cx| ws.save_pr_body(id, cx))),
                )
                .when(conflict, |row| {
                    row.child(
                        button_compact("pr-body-overwrite", s::body_overwrite())
                            .debug_selector(|| "pr-body-overwrite".into())
                            .on_click(
                                cx.listener(move |ws, _, _, cx| ws.overwrite_pr_body(id, cx)),
                            ),
                    )
                })
                .child(
                    button_compact("pr-body-cancel", common::btn_cancel())
                        .debug_selector(|| "pr-body-cancel".into())
                        .disabled(saving)
                        .on_click(cx.listener(move |ws, _, _, cx| ws.cancel_pr_body_edit(id, cx))),
                ),
        )
        .into_any_element()
}

/// The comments on the pull request itself, oldest first.
fn conversation(id: PrDetailId, view: &ConversationView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let (heading, body): (String, Vec<AnyElement>) = match view {
        ConversationView::Loading => (
            s::conversation(),
            vec![note(common::loading(), cx).into_any_element()],
        ),
        ConversationView::Failed(reason) => (
            s::conversation(),
            vec![note(s::conversation_failed(reason), cx).into_any_element()],
        ),
        ConversationView::Listed {
            comments,
            truncated,
        } => {
            let mut body: Vec<AnyElement> = comments
                .iter()
                .map(|comment| comment_block(id, comment, cx))
                .collect();
            if comments.is_empty() {
                body.push(note(s::conversation_empty(), cx).into_any_element());
            }
            if *truncated {
                body.push(note(s::conversation_truncated(), cx).into_any_element());
            }
            let count = if *truncated {
                s::count_capped(comments.len())
            } else {
                comments.len().to_string()
            };
            (s::tab_count(s::conversation(), count), body)
        }
    };
    div()
        .debug_selector(|| "pr-conversation".into())
        .flex()
        .flex_col()
        .gap(px(theme::GAP_LG))
        .pt(px(theme::PAD_LG))
        .border_t_1()
        .border_color(t.border)
        .child(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(heading),
        )
        .children(body)
        .into_any_element()
}

fn comment_block(id: PrDetailId, comment: &CommentView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    super::anchors::mark_comment(div(), id, comment, cx)
        .flex()
        .flex_col()
        .gap(px(theme::GAP_XS))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(comment.author.clone())
                .children(comment.url.clone().map(|url| {
                    let key = format!("pr-conversation-github-{}", comment.id);
                    button_icon(gpui::ElementId::Name(key.into()), icons::OPEN_IN_NEW, cx)
                        .tooltip(git_s::pr_open_on_github())
                        .on_click(move |_, _, cx| cx.open_url(&url))
                })),
        )
        // Keyed by GitHub's id: a re-read keeps each body's parsed state.
        .child(
            crate::ui::markdown(
                gpui::ElementId::Name(format!("pr-conversation-{}", comment.id).into()),
                comment.body.clone(),
            )
            // Written by anyone on a public repository: only web pages open.
            .link_click_handler(super::open_description_link),
        )
        .into_any_element()
}
