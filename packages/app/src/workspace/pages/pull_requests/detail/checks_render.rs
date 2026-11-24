//! Check runs, their logs and the prompt composed from a selected failure.

use daruda_github::CheckStatus;
use gpui::{AnyElement, Context, Hsla, div, prelude::*, px};

use super::render::{send_line, with_slot};
use super::view::{CheckRowView, LogSlot, ReadyView, SendSlot};
use super::{Load, PrDetailId};
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{Disableable as _, Sizable as _, button_compact, button_icon, icons, theme};
use crate::workspace::Workspace;

pub(super) fn checks(id: PrDetailId, ready: &ReadyView, cx: &Context<Workspace>) -> AnyElement {
    let mut ordered: Vec<_> = ready.checks.iter().enumerate().collect();
    ordered.sort_by_key(|(_, row)| row.status != CheckStatus::Failure);
    let rows: Vec<AnyElement> = ordered
        .into_iter()
        .filter(|(_, row)| !ready.failures_only || row.status == CheckStatus::Failure)
        .map(|(ix, row)| check_row(id, ix, row, ready.checks_send, cx))
        .collect();
    div()
        .flex()
        .flex_col()
        .child(send_line(
            Some(ready.checks_summary.clone()),
            ready.checks_send,
            s::send_nothing_checks(),
            move |ws, _, cx| ws.open_pr_handoff(id, super::handoff::Read::FailedChecks, cx),
            "pr-detail-send-checks",
            ready.rerun.as_ref().map(|slot| {
                with_slot(button_compact("pr-detail-rerun", s::action_rerun()), slot)
                    .debug_selector(|| "pr-detail-rerun".into())
                    .on_click(cx.listener(move |ws, _, _, cx| ws.rerun_pr_detail_checks(id, cx)))
                    .into_any_element()
            }),
            cx,
        ))
        .when(ready.failures_only, |panel| {
            panel.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(theme::GAP_SM))
                    .child(
                        button_compact("pr-show-all-checks", s::show_all_checks()).on_click(
                            cx.listener(move |ws, _, _, cx| {
                                ws.select_pr_detail_tab(id, super::PrTab::Checks, cx)
                            }),
                        ),
                    )
                    .children(ready.truncated_checks_url.clone().map(|url| {
                        button_compact("pr-open-all-checks", s::all_checks_github())
                            .on_click(move |_, _, cx| cx.open_url(&url))
                    })),
            )
        })
        .children(rows)
        .into_any_element()
}

fn check_row(
    id: PrDetailId,
    ix: usize,
    row: &CheckRowView,
    send: SendSlot,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let (glyph, color) = status_glyph(row.status, cx);
    let toggle = match &row.log {
        LogSlot::None => None,
        LogSlot::Closed(job) | LogSlot::Open(job, _) => {
            let job = *job;
            let open = matches!(row.log, LogSlot::Open(..));
            Some(
                button_icon(
                    ("pr-check-log", ix),
                    if open {
                        icons::EXPAND_MORE
                    } else {
                        icons::CHEVRON_RIGHT
                    },
                    cx,
                )
                .tooltip(if open { s::hide_log() } else { s::show_log() })
                .debug_selector(move || format!("pr-check-log-{ix}"))
                .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_pr_check_log(id, job, cx))),
            )
        }
    };
    let line = div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_LG))
        .min_h(px(theme::CONTROL_TARGET_SIZE))
        .text_size(px(theme::FONT_SIZE_MD))
        .px(px(theme::LIST_ROW_PAD_X))
        .py(px(theme::LIST_ROW_PAD_Y))
        .rounded(px(theme::LIST_ROW_RADIUS))
        .hover(|style| style.bg(t.overlay_hover))
        .child(
            icons::icon(glyph)
                .with_size(px(theme::CONTROL_ICON_SIZE))
                .text_color(color),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(theme::GAP_LG))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_body)
                        .child(row.name.clone()),
                )
                .children(row.context.clone().map(|context| {
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_muted)
                        .child(context)
                })),
        )
        .children(row.duration.clone().map(|duration| {
            div()
                .flex_none()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_subtle)
                .child(duration)
        }))
        // Each icon keeps its column, so durations line up row to row.
        .child(
            div()
                .flex_none()
                .w(px(theme::CONTROL_TARGET_SIZE))
                .children(row.rerun.as_ref().map(|(run, slot)| {
                    let run = *run;
                    with_slot(
                        button_icon(("pr-check-rerun", ix), icons::REFRESH, cx),
                        slot,
                    )
                    .tooltip(s::rerun_check())
                    .debug_selector(move || format!("pr-check-rerun-{ix}"))
                    .on_click(cx.listener(move |ws, _, _, cx| ws.rerun_pr_check_run(id, run, cx)))
                })),
        )
        .child(
            div()
                .flex_none()
                .w(px(theme::CONTROL_TARGET_SIZE))
                .children(row.url.clone().map(|url| {
                    button_icon(("pr-check-open", ix), icons::OPEN_IN_NEW, cx)
                        .tooltip(s::open_check())
                        .debug_selector(move || format!("pr-check-open-{ix}"))
                        .on_click(move |_, _, cx| cx.open_url(&url))
                })),
        )
        .child(
            div()
                .flex_none()
                .w(px(theme::CONTROL_TARGET_SIZE))
                .children(toggle),
        );
    let log = match &row.log {
        LogSlot::Open(_, load) => Some(log_block(load, cx)),
        LogSlot::None | LogSlot::Closed(_) => None,
    };
    let transfer = match &row.log {
        LogSlot::Open(job, Load::Ready(_)) => {
            let job = *job;
            Some(
                button_compact(("pr-send-log", ix), s::send_log())
                    .disabled(!send.enabled())
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.open_pr_handoff(id, super::handoff::Read::FailedJob(job), cx)
                    })),
            )
        }
        _ => None,
    };
    div()
        .flex()
        .flex_col()
        .child(line)
        .children(log)
        .children(transfer)
        .into_any_element()
}

/// The failed step's log, cleaned, under its check.
fn log_block(load: &Load<String>, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let (text, color) = match load {
        Load::Loading => (common::loading(), t.text_muted),
        Load::Ready(log) => (log.clone(), t.text_body),
        Load::Failed(reason) => (s::log_failed(reason), t.banner_error_text),
    };
    div()
        .mx(px(theme::LIST_ROW_PAD_X))
        .mb(px(theme::PAD_SM))
        .p(px(theme::PAD_STANDARD))
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(t.border)
        .bg(t.md_code_block_bg)
        .font_family(theme::FONT_FAMILY_MONOSPACE)
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(color)
        .child(text)
        .into_any_element()
}

fn status_glyph(status: CheckStatus, cx: &Context<Workspace>) -> (&'static str, Hsla) {
    let t = theme::current(cx);
    match status {
        CheckStatus::Failure => (icons::WARNING, t.banner_error_text),
        CheckStatus::Pending => (icons::RADIO_OFF, t.banner_warning_text),
        CheckStatus::Success => (icons::CHECK_CIRCLE, t.banner_success_text),
        CheckStatus::Ignored => (icons::RADIO_OFF, t.text_subtle),
    }
}

#[cfg(test)]
mod tests {}
