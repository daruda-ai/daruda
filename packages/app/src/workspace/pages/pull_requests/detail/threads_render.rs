//! The review-threads tab: each unresolved thread as a line, expanded to
//! its comments in full, with the way to its lines and to GitHub.

use gpui::{AnyElement, Context, Entity, div, prelude::*, px};

use super::render::{note, send_line};
use super::thread_actions::{ThreadAction, ThreadOp};
use super::view::{CommentView, ReadyView, ThreadRowView, ThreadsView};
use super::{PrDetailId, PrTab};
use crate::github::DraftKeeping;
use crate::surface::strings::{common, git as git_s, pull_requests as s};
use crate::ui::{
    Badge, Disableable as _, TextareaState, button_compact, button_icon, button_with_icon, icons,
    markdown_editor, theme,
};
use crate::workspace::Workspace;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ThreadPlace {
    List,
    Inline,
    FileConversations,
    BeyondLimit,
}

pub(super) fn threads(id: PrDetailId, ready: &ReadyView, cx: &Context<Workspace>) -> AnyElement {
    let send = send_line(
        None,
        ready.threads_send,
        s::send_nothing_threads(),
        move |ws, _, cx| ws.open_pr_handoff(id, super::handoff::Read::OpenThreads, cx),
        "pr-detail-send-threads",
        None,
        cx,
    );
    let body: Vec<AnyElement> = match &ready.threads {
        ThreadsView::Loading => vec![note(common::loading(), cx).into_any_element()],
        ThreadsView::Failed(reason) => {
            vec![note(s::threads_failed(reason), cx).into_any_element()]
        }
        ThreadsView::Listed {
            rows,
            truncated,
            keeping,
            orphan_drafts,
        } => {
            let mut body: Vec<AnyElement> = rows
                .iter()
                .enumerate()
                .map(|(index, row)| thread_row(id, index, row, *keeping, ThreadPlace::List, cx))
                .collect();
            if rows.is_empty() {
                body.push(note(s::threads_empty(), cx).into_any_element());
            }
            if *truncated {
                body.push(note(s::threads_truncated(), cx).into_any_element());
            }
            if !orphan_drafts.is_empty() {
                body.push(orphan_drafts_list(id, orphan_drafts, cx));
            }
            body
        }
    };
    div()
        .flex()
        .flex_col()
        .child(send)
        .children(body)
        .into_any_element()
}

/// A thread's line: where, who and the first words; pressing it shows the
/// thread in full.
pub(super) fn thread_row(
    id: PrDetailId,
    index: usize,
    row: &ThreadRowView,
    keeping: DraftKeeping,
    place: ThreadPlace,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let thread = row.id.clone();
    let summary = div()
        .id(("pr-thread", index))
        .debug_selector(move || format!("pr-thread-{index}"))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_XS))
        .px(px(theme::LIST_ROW_PAD_X))
        .py(px(theme::PAD_SM))
        .rounded(px(theme::LIST_ROW_RADIUS))
        .cursor_pointer()
        .hover(|style| style.bg(t.overlay_hover))
        .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_pr_thread(id, &thread, cx)))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_LG))
                .min_w_0()
                .when(place == ThreadPlace::Inline, |line| {
                    line.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(theme::FONT_SIZE_SM))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(t.text_body)
                            .child(row.author.clone()),
                    )
                })
                .when(place != ThreadPlace::Inline, |line| {
                    line.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::FONT_FAMILY_MONOSPACE)
                            .text_size(px(theme::FONT_SIZE_SM))
                            .text_color(t.text_body)
                            .child(row.location.clone()),
                    )
                })
                .when_some(
                    row.target.as_ref().filter(|_| place == ThreadPlace::Inline),
                    |line, target| {
                        line.when(target.start != target.end, |line| {
                            line.child(
                                div()
                                    .flex_none()
                                    .font_family(theme::FONT_FAMILY_MONOSPACE)
                                    .text_size(px(theme::FONT_SIZE_SM))
                                    .text_color(t.text_muted)
                                    .child(s::thread_inline_range(target.start, target.end)),
                            )
                        })
                        .child(
                            Badge::new(match target.side {
                                super::diff_layout::Side::Old => s::thread_base_side(),
                                super::diff_layout::Side::New => s::thread_head_side(),
                            })
                            .text_color(t.text_muted),
                        )
                    },
                )
                .when(row.outdated, |line| {
                    line.child(Badge::new(s::thread_outdated()).text_color(t.text_muted))
                })
                .when(row.resolved_here, |line| {
                    line.child(Badge::new(s::thread_resolved()).text_color(t.banner_success_text))
                })
                .when(row.draft && row.reply.is_none(), |line| {
                    line.child(Badge::new(s::draft_badge()).text_color(t.banner_warning_text))
                })
                .child(div().flex_1())
                .children(row.replies.clone().map(|replies| {
                    div()
                        .flex_none()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_subtle)
                        .child(replies)
                })),
        )
        .when(row.comments.is_none(), |summary| {
            summary.child(
                div()
                    .flex()
                    .gap(px(theme::GAP_SM))
                    .min_w_0()
                    .text_size(px(theme::FONT_SIZE_SM))
                    .when(place != ThreadPlace::Inline, |line| {
                        line.child(
                            div()
                                .flex_none()
                                .text_color(t.text_muted)
                                .child(row.author.clone()),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(t.text_subtle)
                            .child(row.excerpt.clone()),
                    ),
            )
        });
    div()
        .flex()
        .flex_col()
        .child(summary)
        .children(
            row.comments
                .as_ref()
                .map(|comments| expanded(id, index, row, comments, keeping, place, cx)),
        )
        .into_any_element()
}

/// A thread in full: every comment as Markdown, and its way out.
fn expanded(
    id: PrDetailId,
    index: usize,
    row: &ThreadRowView,
    comments: &[CommentView],
    keeping: DraftKeeping,
    place: ThreadPlace,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let thread = row.id.clone();
    let navigates = place != ThreadPlace::List || row.target.is_some();
    let actions = div()
        .flex()
        .flex_wrap()
        .gap(px(theme::GAP_SM))
        // A jump here from the diff keeps the keyboard with this thread even
        // when it has no way back to the diff (outdated, or at another head).
        .when(!navigates, |actions| {
            actions.when_some(row.navigation_focus.as_ref(), |actions, handle| {
                actions.track_focus(handle)
            })
        })
        .when(navigates, |actions| {
            let (dest, label) =
                if matches!(place, ThreadPlace::Inline | ThreadPlace::FileConversations) {
                    (PrTab::Threads, s::thread_show_in_threads())
                } else {
                    (PrTab::Files, s::thread_show_in_diff())
                };
            actions.child(
                button_compact(("pr-thread-diff", index), label)
                    .debug_selector(move || format!("pr-thread-diff-{index}"))
                    .tab_stop(true)
                    .when_some(row.navigation_focus.as_ref(), |button, handle| {
                        button.track_focus(handle)
                    })
                    .on_click(cx.listener(move |ws, _, window, cx| {
                        ws.jump_pr_thread(id, &thread, dest, window, cx)
                    })),
            )
        })
        .when(row.outdated, |line| {
            line.child(note(s::thread_outdated_context(), cx))
        })
        .children(row.url.clone().map(|url| {
            button_with_icon(
                ("pr-thread-github", index),
                git_s::pr_open_on_github(),
                icons::OPEN_IN_NEW,
            )
            .on_click(move |_, _, cx| cx.open_url(&url))
        }))
        .children(thread_actions(id, index, row, cx));
    div()
        .debug_selector(move || format!("pr-thread-full-{index}"))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_LG))
        .ml(px(theme::LIST_ROW_PAD_X))
        .pl(px(theme::PAD_LG))
        .py(px(theme::PAD_SM))
        .border_l_1()
        .border_color(t.border)
        .children(comments.iter().map(|comment| {
            super::anchors::mark_comment(div(), comment)
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
                            let key = format!("pr-thread-comment-github-{}", comment.id);
                            button_icon(gpui::ElementId::Name(key.into()), icons::OPEN_IN_NEW, cx)
                                .tooltip(git_s::pr_open_on_github())
                                .on_click(move |_, _, cx| cx.open_url(&url))
                        })),
                )
                // Keyed by GitHub's id, so a refresh that drops a thread
                // does not hand another comment this one's parsed state.
                .child(
                    crate::ui::markdown(
                        gpui::ElementId::Name(format!("pr-thread-comment-{}", comment.id).into()),
                        comment.body.clone(),
                    )
                    // Written by any reviewer: only web pages open.
                    .link_click_handler(super::open_description_link),
                )
        }))
        .children(
            row.reply
                .as_ref()
                .map(|reply| reply_box(id, index, row, reply, keeping, cx)),
        )
        .children(row.op.as_ref().map(|op| op_line(op, cx)))
        .child(actions)
        .into_any_element()
}

/// Reply and resolve, or unresolve a thread resolved from here; off while
/// an action runs on the thread.
fn thread_actions(
    id: PrDetailId,
    index: usize,
    row: &ThreadRowView,
    cx: &Context<Workspace>,
) -> Vec<AnyElement> {
    // A reply being checked holds the thread's other actions too (`begin`).
    let running = matches!(
        row.op,
        Some(ThreadOp::Running(_) | ThreadOp::Checking(_) | ThreadOp::Unconfirmed(_))
    );
    let thread = row.id.clone();
    let mut actions = Vec::new();
    if row.reply.is_none() && !row.resolved_here && row.may.reply {
        let thread = thread.clone();
        actions.push(
            button_compact(("pr-thread-reply", index), s::thread_reply())
                .debug_selector(move || format!("pr-thread-reply-{index}"))
                .disabled(running)
                .on_click(
                    cx.listener(move |ws, _, window, cx| ws.open_pr_reply(id, &thread, window, cx)),
                )
                .into_any_element(),
        );
    }
    let resolved = row.resolved_here;
    let may = if resolved {
        row.may.unresolve
    } else {
        row.may.resolve
    };
    if !may {
        return actions;
    }
    actions.push(
        button_compact(
            ("pr-thread-resolve", index),
            if resolved {
                s::thread_unresolve()
            } else {
                s::thread_resolve()
            },
        )
        .debug_selector(move || format!("pr-thread-resolve-{index}"))
        .disabled(running)
        .on_click(
            cx.listener(move |ws, _, _, cx| ws.set_pr_thread_resolved(id, &thread, !resolved, cx)),
        )
        .into_any_element(),
    );
    actions
}

/// The reply being written, with Post and Cancel, and, when whether it
/// posted could not be confirmed, a way to say it did not.
fn reply_box(
    id: PrDetailId,
    index: usize,
    row: &ThreadRowView,
    reply: &Entity<TextareaState>,
    keeping: DraftKeeping,
    cx: &Context<Workspace>,
) -> AnyElement {
    let running = matches!(row.op, Some(ThreadOp::Running(_)));
    let checking = matches!(
        row.op,
        Some(ThreadOp::Checking(_) | ThreadOp::Unconfirmed(_))
    );
    let unconfirmed = matches!(row.op, Some(ThreadOp::Unconfirmed(_)));
    // The box notifies the detail as it is typed in, so this follows it.
    let empty = reply.read(cx).value().trim().is_empty();
    let (post, cancel, not_posted) = (row.id.clone(), row.id.clone(), row.id.clone());
    div()
        .debug_selector(move || format!("pr-thread-reply-box-{index}"))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .child(markdown_editor(reply, cx))
        .children(super::render::keeping_note(keeping, cx))
        .child(
            div()
                .flex()
                .gap(px(theme::GAP_SM))
                .child(
                    button_compact(("pr-thread-post", index), s::thread_post_reply())
                        .debug_selector(move || format!("pr-thread-post-{index}"))
                        .disabled(running || checking || empty)
                        .on_click(cx.listener(move |ws, _, _, cx| ws.post_pr_reply(id, &post, cx))),
                )
                .when(unconfirmed, |buttons| {
                    buttons.child(
                        button_compact(
                            ("pr-thread-not-posted", index),
                            s::thread_reply_not_posted(),
                        )
                        .debug_selector(move || format!("pr-thread-not-posted-{index}"))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.mark_pr_reply_not_posted(id, &not_posted, cx)
                        })),
                    )
                })
                .child(
                    button_compact(("pr-thread-cancel", index), s::thread_cancel_reply())
                        .disabled(running)
                        .on_click(
                            cx.listener(move |ws, _, _, cx| ws.cancel_pr_reply(id, &cancel, cx)),
                        ),
                ),
        )
        .into_any_element()
}

/// What the thread's action is doing, or why it failed.
fn op_line(op: &ThreadOp, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let (text, color) = match op {
        ThreadOp::Running(action) => (
            match action {
                ThreadAction::Replying => s::thread_replying(),
                ThreadAction::Resolving => s::thread_resolving(),
                ThreadAction::Unresolving => s::thread_unresolving(),
            },
            t.text_muted,
        ),
        ThreadOp::Checking(_) => (s::thread_reply_checking(), t.text_muted),
        ThreadOp::Unconfirmed(_) => (s::thread_reply_unconfirmed(), t.banner_error_text),
        ThreadOp::Failed(action, reason) => (
            s::thread_action_failed(action.failure_title(), reason),
            t.banner_error_text,
        ),
    };
    div()
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// Reply drafts kept for threads no longer listed, each shown so it can be
/// copied, and discarded one by one only when asked.
fn orphan_drafts_list(
    id: PrDetailId,
    drafts: &[(String, String)],
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    div()
        .debug_selector(|| "pr-thread-orphan-drafts".into())
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .py(px(theme::PAD_SM))
        .text_size(px(theme::FONT_SIZE_SM))
        .child(
            div()
                .text_color(t.banner_warning_text)
                .child(s::orphan_reply_drafts(drafts.len())),
        )
        .children(drafts.iter().enumerate().map(|(index, (thread, text))| {
            let (copied, thread) = (text.clone(), thread.clone());
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_body)
                        .child(text.lines().next().unwrap_or_default().to_owned()),
                )
                .child(
                    button_icon(("pr-orphan-copy", index), icons::COPY, cx)
                        .tooltip(s::orphan_draft_copy())
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied.clone()))
                        }),
                )
                .child(
                    button_compact(
                        ("pr-orphan-discard", index),
                        s::orphan_reply_drafts_discard(),
                    )
                    .debug_selector(move || format!("pr-orphan-discard-{index}"))
                    .on_click(
                        cx.listener(move |ws, _, _, cx| ws.discard_pr_reply_draft(id, &thread, cx)),
                    ),
                )
        }))
        .into_any_element()
}
