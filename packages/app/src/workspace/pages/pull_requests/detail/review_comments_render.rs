//! Comments on lines for a review not yet submitted: the list of those
//! pending, and the composer of the one being written.

use gpui::{AnyElement, Context, div, prelude::*, px};

use super::PrDetailId;
use super::view::{ComposerView, ReviewCommentsView};
use crate::surface::strings::pull_requests as s;
use crate::ui::{Badge, Disableable as _, button_compact, markdown_editor, theme};
use crate::workspace::Workspace;

/// The comment being written, the pending comments (each with Edit and
/// Discard), and the review they go out with.
pub(super) fn pending(
    id: PrDetailId,
    view: &ReviewCommentsView,
    cx: &Context<Workspace>,
) -> Option<AnyElement> {
    let t = theme::current(cx);
    if let Some(draft) = &view.composer {
        return Some(composer(id, draft, view, cx));
    }
    let review = view
        .review
        .as_ref()
        .map(|review| super::review_submit_render::review(id, review, view.keeping, cx));
    if view.pending.is_empty() {
        return review;
    }
    let rows = view.pending.iter().map(|comment| {
        let (edit, discard, key) = (comment.id, comment.id, comment.id as usize);
        // Where on one line with its controls, what it says under it: a
        // long path must not crowd the text out.
        div()
            .debug_selector(move || format!("pr-pending-comment-{key}"))
            .flex()
            .flex_col()
            .gap(px(theme::GAP_XS))
            .text_size(px(theme::FONT_SIZE_SM))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(theme::GAP_SM))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme::FONT_FAMILY_MONOSPACE)
                            .text_color(t.text_muted)
                            .child(comment.reference.clone()),
                    )
                    .when(comment.excluded, |row| {
                        row.child(Badge::new(s::comment_excluded()).text_color(t.text_muted))
                    })
                    .when(comment.suggests, |row| {
                        row.child(
                            Badge::new(s::comment_suggestion_badge()).text_color(t.text_muted),
                        )
                    })
                    .when(comment.outdated, |row| {
                        row.child(
                            Badge::new(s::comment_outdated()).text_color(t.banner_warning_text),
                        )
                    })
                    .when(comment.unanchored, |row| {
                        row.child(
                            Badge::new(s::comment_unanchored()).text_color(t.banner_warning_text),
                        )
                    })
                    .child(
                        button_compact(("pr-pending-edit", key), s::comment_edit())
                            .debug_selector(move || format!("pr-pending-edit-{key}"))
                            .disabled(view.locked)
                            .on_click(cx.listener(move |ws, _, window, cx| {
                                ws.edit_pr_line_comment(id, edit, window, cx)
                            })),
                    )
                    .child(
                        button_compact(("pr-pending-discard", key), s::comment_discard())
                            .debug_selector(move || format!("pr-pending-discard-{key}"))
                            .disabled(view.locked)
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.discard_pr_line_comment(id, discard, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_body)
                    .child(comment.excerpt.clone()),
            )
    });
    Some(
        div()
            .debug_selector(|| "pr-pending-comments".into())
            .flex()
            .flex_col()
            .gap(px(theme::GAP_SM))
            .p(px(theme::PAD_LG))
            .rounded(px(theme::RADIUS_SM))
            .border_1()
            .border_color(t.border)
            .children(review)
            .when(view.review.is_none(), |panel| {
                panel.child(super::render::note(
                    s::pending_comments(view.pending.len()),
                    cx,
                ))
            })
            .child(
                div()
                    .id("pr-pending-list")
                    .flex()
                    .flex_col()
                    .max_h(px(theme::PR_PENDING_LIST_MAX_H))
                    .overflow_y_scroll()
                    .gap(px(theme::GAP_SM))
                    .children(rows),
            )
            .into_any_element(),
    )
}

/// The comment being written: its lines, text, optional replacement text,
/// Done and Discard.
fn composer(
    id: PrDetailId,
    view: &ComposerView,
    comments: &ReviewCommentsView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let blocking = view.outdated || view.unanchored;
    let reviewing = comments.review.is_some();
    div()
        .debug_selector(|| "pr-line-composer".into())
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .pb(px(theme::PAD_LG))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_color(t.text_muted)
                        .child(view.reference.clone()),
                )
                .when(view.outdated, |row| {
                    row.child(Badge::new(s::comment_outdated()).text_color(t.banner_warning_text))
                })
                .when(view.unanchored, |row| {
                    row.child(Badge::new(s::comment_unanchored()).text_color(t.banner_warning_text))
                }),
        )
        .child(markdown_editor(&view.body, cx))
        .children(view.suggestion.as_ref().map(|suggestion| {
            div()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_XS))
                .child(
                    div()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_muted)
                        .child(s::comment_suggestion_label()),
                )
                .child(markdown_editor(suggestion, cx))
        }))
        .children(super::render::keeping_note(comments.keeping, cx))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(theme::GAP_SM))
                .when(blocking, |row| {
                    row.child(
                        button_compact("pr-composer-reanchor", s::comment_reanchor())
                            .debug_selector(|| "pr-composer-reanchor".into())
                            .disabled(!view.can_reanchor)
                            .tooltip(s::comment_reanchor_hint())
                            .on_click(
                                cx.listener(move |ws, _, _, cx| ws.reanchor_pr_comment(id, cx)),
                            ),
                    )
                })
                .when_some(view.id.filter(|_| reviewing && blocking), |row, comment| {
                    row.child(
                        button_compact("pr-composer-exclude", s::comment_exclude())
                            .debug_selector(|| "pr-composer-exclude".into())
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.exclude_pr_review_comment(id, comment, cx)
                            })),
                    )
                })
                .child(
                    button_compact(
                        "pr-composer-done",
                        if reviewing {
                            s::review_return()
                        } else {
                            s::comment_done()
                        },
                    )
                    .debug_selector(|| "pr-composer-done".into())
                    .on_click(cx.listener(move |ws, _, _, cx| ws.close_pr_line_comment(id, cx))),
                )
                .when(view.can_suggest, |row| {
                    row.child(
                        button_compact(
                            "pr-composer-suggest",
                            if view.suggestion.is_some() {
                                s::comment_suggestion_remove()
                            } else {
                                s::comment_suggestion_add()
                            },
                        )
                        .debug_selector(|| "pr-composer-suggest".into())
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.toggle_pr_suggestion(id, window, cx)
                        })),
                    )
                })
                .child(
                    button_compact("pr-composer-discard", s::comment_discard())
                        .debug_selector(|| "pr-composer-discard".into())
                        .on_click(cx.listener(move |ws, _, _, cx| ws.discard_pr_composer(id, cx))),
                ),
        )
        .into_any_element()
}
