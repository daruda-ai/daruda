//! Under the reviewers: unask one not yet reviewed, ask others by name, and
//! how the last change went.

use gpui::{AnyElement, Context, div, prelude::*, px};

use super::PrDetailId;
use super::reviewers::ReviewerOp;
use super::view::RequestsView;
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{Disableable as _, button_compact, button_icon, icons, input, theme};
use crate::workspace::Workspace;

pub(super) fn requests(id: PrDetailId, view: &RequestsView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let running = view.op == Some(ReviewerOp::Running);
    let line = view.op.as_ref().map(|op| match op {
        ReviewerOp::Running => (s::reviewers_running(), t.text_muted),
        ReviewerOp::Invalid(name) => (s::reviewer_invalid(name), t.banner_warning_text),
        ReviewerOp::Author(name) => (s::reviewer_author(name), t.banner_warning_text),
        ReviewerOp::Failed(reason) => (
            s::reviewers_failed_line(s::reviewers_failed(), reason),
            t.banner_error_text,
        ),
    });
    let ask = match &view.input {
        Some(state) => div()
            .flex()
            .items_center()
            .gap(px(theme::GAP_SM))
            .child(div().flex_1().min_w_0().child(input(state, cx, ())))
            .child(
                button_compact("pr-reviewers-request", s::reviewers_request())
                    .debug_selector(|| "pr-reviewers-request".into())
                    .disabled(running)
                    .on_click(cx.listener(move |ws, _, _, cx| ws.request_pr_reviewers(id, cx))),
            )
            .child(
                button_icon("pr-reviewers-close", icons::CLOSE, cx)
                    .tooltip(common::btn_cancel())
                    .disabled(running)
                    .on_click(cx.listener(move |ws, _, _, cx| ws.close_pr_reviewer_input(id, cx))),
            )
            .into_any_element(),
        None => button_compact("pr-reviewers-open", s::reviewers_add())
            .debug_selector(|| "pr-reviewers-open".into())
            .disabled(running)
            .on_click(
                cx.listener(move |ws, _, window, cx| ws.open_pr_reviewer_input(id, window, cx)),
            )
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .children(view.removable.iter().enumerate().map(|(index, name)| {
            let unask = name.clone();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(theme::GAP_SM))
                .text_size(px(theme::FONT_SIZE_SM))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_subtle)
                        .child(name.clone()),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(t.text_muted)
                        .child(s::reviewer_requested()),
                )
                .child(
                    button_icon(("pr-reviewer-remove", index), icons::CLOSE, cx)
                        .debug_selector(move || format!("pr-reviewer-remove-{index}"))
                        .tooltip(s::reviewer_remove(name))
                        .disabled(running)
                        .on_click(
                            cx.listener(move |ws, _, _, cx| ws.remove_pr_reviewer(id, &unask, cx)),
                        ),
                )
        }))
        .child(ask)
        .children(line.map(|(text, color)| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(color)
                .child(text)
        }))
        .into_any_element()
}
