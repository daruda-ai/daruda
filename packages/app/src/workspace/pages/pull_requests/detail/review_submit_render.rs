//! The review being written: its text, its verdict, Submit, and how
//! submitting stands.

use daruda_github::ReviewEvent;
use gpui::{AnyElement, Context, div, prelude::*, px};

use super::PrDetailId;
use super::review_submit::{SubmitBlock, SubmitState};
use super::view::ReviewView;
use crate::github::DraftKeeping;
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{
    Disableable as _, Selectable as _, button, button_compact, button_group, button_primary,
    markdown_editor, theme,
};
use crate::workspace::Workspace;

const EVENTS: [ReviewEvent; 3] = [
    ReviewEvent::Comment,
    ReviewEvent::Approve,
    ReviewEvent::RequestChanges,
];

pub(super) fn review(
    id: PrDetailId,
    view: &ReviewView,
    keeping: DraftKeeping,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let submitting = !view.state.editable();
    let unknown = matches!(view.state, SubmitState::Unknown { .. });
    let line = match &view.state {
        SubmitState::Writing => None,
        SubmitState::Submitting => Some((s::review_submitting(), t.text_muted)),
        SubmitState::Blocked(block) => Some((
            match block {
                SubmitBlock::Unanchored(count) => s::review_blocked_unanchored(*count),
                SubmitBlock::Empty => s::review_blocked_empty(),
                SubmitBlock::OwnPullRequest => s::review_blocked_own(),
                SubmitBlock::NotRead => s::review_blocked_not_read(),
            },
            t.banner_warning_text,
        )),
        SubmitState::Failed(reason) => Some((
            s::review_failed_line(s::review_submit_failed(), reason),
            t.banner_error_text,
        )),
        SubmitState::Unknown { reason, .. } => {
            Some((s::review_unknown(reason), t.banner_warning_text))
        }
        SubmitState::Checking => Some((s::review_checking(), t.text_muted)),
    };
    let leave_out = matches!(view.state, SubmitState::Blocked(SubmitBlock::Unanchored(_)));
    let notice = line.map(|(text, color)| {
        div()
            .text_size(px(theme::FONT_SIZE_SM))
            .text_color(color)
            .child(text)
    });
    div()
        .debug_selector(|| "pr-review".into())
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .pt(px(theme::PAD_SM))
        .border_t_1()
        .border_color(t.border)
        .child(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(s::review_heading()),
        )
        .children(notice)
        .when(leave_out, |panel| {
            panel.children(view.first_blocker.map(|comment| {
                div().flex().items_start().child(
                    button_primary("pr-review-first-blocker", s::review_first_blocker())
                        .debug_selector(|| "pr-review-first-blocker".into())
                        .on_click(cx.listener(move |ws, _, window, cx| {
                            ws.edit_pr_line_comment(id, comment, window, cx)
                        })),
                )
            }))
        })
        .child(markdown_editor(&view.body, cx).disabled(submitting))
        .children(super::render::keeping_note(keeping, cx))
        .child(
            button_group("pr-review-event", cx)
                .children(EVENTS.into_iter().enumerate().map(|(index, event)| {
                    button(
                        ("pr-review-event", index),
                        match event {
                            ReviewEvent::Comment => s::review_comment(),
                            ReviewEvent::Approve => s::review_approve(),
                            ReviewEvent::RequestChanges => s::review_request_changes(),
                        },
                    )
                    .selected(view.event == event)
                    .disabled(submitting)
                    .debug_selector(move || format!("pr-review-event-{index}"))
                }))
                .on_click(cx.listener(move |ws, picked: &Vec<usize>, _, cx| {
                    ws.pick_pr_review_event(id, picked.first().copied(), cx)
                })),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(theme::GAP_SM))
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(s::review_comments_included(view.included_comments))
                .when(view.excluded_comments > 0, |line| {
                    line.child(s::review_comments_excluded(view.excluded_comments))
                }),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(theme::GAP_SM))
                .when(unknown, |row| {
                    row.child(
                        button_primary("pr-review-check", s::review_check())
                            .debug_selector(|| "pr-review-check".into())
                            .on_click(cx.listener(move |ws, _, _, cx| ws.check_pr_review(id, cx))),
                    )
                })
                .when(!leave_out, |row| {
                    row.child(
                        button_primary("pr-review-submit", s::review_submit())
                            .debug_selector(|| "pr-review-submit".into())
                            .disabled(submitting)
                            .on_click(cx.listener(move |ws, _, _, cx| ws.submit_pr_review(id, cx))),
                    )
                })
                .when(leave_out, |row| {
                    row.child(
                        button_compact("pr-review-leave-out", s::review_leave_out())
                            .debug_selector(|| "pr-review-leave-out".into())
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.submit_pr_review_without_unanchored(id, cx)
                            })),
                    )
                })
                .child(
                    button_compact("pr-review-close", common::btn_cancel())
                        .disabled(submitting)
                        .on_click(cx.listener(move |ws, _, _, cx| ws.close_pr_review(id, cx))),
                ),
        )
        .into_any_element()
}
