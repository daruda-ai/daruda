//! The active lane's pull request under the Git Changes header: its number,
//! title and checks, or why none can be shown.

use daruda_github::{CheckRollup, PrState};
use gpui::{AnyElement, ClickEvent, Context, IntoElement, div, prelude::*, px};

use crate::surface::strings::git as s;
use crate::ui::theme;
use crate::ui::{Disableable as _, Sizable as _};
use crate::workspace::github_ops::strip::PrStrip;
use crate::workspace::github_ops::tone::PrTone;
use crate::workspace::layout::{Dock, LeftDockSnapshot};

/// The glyph and color a check tone shows, or nothing for a quiet one.
/// Shared with the lane row badge so both read the same.
pub(in crate::workspace::left_dock) fn tone_glyph(
    tone: PrTone,
    cx: &gpui::App,
) -> Option<(&'static str, gpui::Hsla)> {
    let t = theme::current(cx);
    match tone {
        PrTone::Passing => Some((crate::ui::icons::CHECK_CIRCLE, t.banner_success_text)),
        PrTone::Failing => Some((crate::ui::icons::WARNING, t.banner_error_text)),
        PrTone::Running => Some((crate::ui::icons::RADIO_OFF, t.banner_warning_text)),
        PrTone::Quiet => None,
    }
}

fn checks_label(checks: CheckRollup) -> Option<String> {
    match checks {
        CheckRollup::Passing => Some(s::pr_checks_passing()),
        CheckRollup::Failing { failed } => Some(s::pr_checks_failing(failed)),
        CheckRollup::Pending => Some(s::pr_checks_running()),
        CheckRollup::None => None,
    }
}

fn state_label(state: PrState) -> Option<String> {
    match state {
        PrState::Open => None,
        PrState::Draft => Some(s::pr_state_draft()),
        PrState::Merged => Some(s::pr_state_merged()),
        PrState::Closed => Some(s::pr_state_closed()),
    }
}

/// The strip, or `None` when there is nothing to say.
pub(super) fn pr_strip(snap: &LeftDockSnapshot, cx: &mut Context<Dock>) -> Option<AnyElement> {
    let t = theme::current(cx);
    let (muted, subtle) = (t.text_muted, t.text_subtle);
    let metrics = snap.metrics;
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(metrics.space(theme::GIT_REMOTE_BTN_GAP)))
        .px(px(metrics.space(theme::GIT_HEADER_PAD_X)))
        .pb(px(metrics.space(theme::GIT_HEADER_PAD_Y)))
        .text_size(px(metrics.text(theme::GIT_BADGE_FONT_SIZE)));
    let workspace = snap.workspace.clone();
    match &snap.pr_strip {
        PrStrip::Hidden => None,
        PrStrip::NoPr => {
            let create = crate::ui::button_compact_sized("pr-create", s::pr_create_btn(), metrics)
                .disabled(snap.git_op_in_flight)
                .on_click(cx.listener(move |_dock, _: &ClickEvent, window, cx| {
                    if let Some(ws) = workspace.upgrade() {
                        ws.update(cx, |ws, cx| ws.open_create_pr_modal(window, cx));
                    }
                }));
            Some(row.justify_end().child(create).into_any_element())
        }
        PrStrip::Missing { host } | PrStrip::LoggedOut { host } => {
            let message = if matches!(snap.pr_strip, PrStrip::Missing { .. }) {
                s::pr_gh_missing()
            } else {
                s::pr_gh_logged_out()
            };
            let host = host.clone();
            let recheck = crate::ui::button_compact_sized("pr-recheck", s::pr_recheck(), metrics)
                .on_click(cx.listener(move |_dock, _: &ClickEvent, _window, cx| {
                    if let Some(ws) = workspace.upgrade() {
                        ws.update(cx, |ws, cx| ws.recheck_gh(&host, cx));
                    }
                }));
            // The hint wraps on its own line; a narrow dock must not push
            // the button out of reach.
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(metrics.space(theme::GIT_REMOTE_BTN_GAP)))
                    .px(px(metrics.space(theme::GIT_HEADER_PAD_X)))
                    .pb(px(metrics.space(theme::GIT_HEADER_PAD_Y)))
                    .text_size(px(metrics.text(theme::GIT_BADGE_FONT_SIZE)))
                    .child(div().w_full().text_color(subtle).child(message))
                    .child(div().flex().flex_row().justify_end().child(recheck))
                    .into_any_element(),
            )
        }
        PrStrip::Linked(link) => {
            let pr = &link.pr;
            let glyph = tone_glyph(PrTone::of(pr), cx);
            let open = crate::ui::button_icon_sized(
                "pr-open-on-github",
                crate::ui::icons::OPEN_IN_NEW,
                metrics,
                cx,
            )
            .tooltip(s::pr_open_on_github())
            .on_click(cx.listener(move |_dock, _: &ClickEvent, _window, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.open_pr_in_browser(cx));
                }
            }));
            let title = div()
                .flex_1()
                .min_w_0()
                .text_ellipsis()
                .text_color(muted)
                .child(format!("{} {}", s::pr_number(pr.number), pr.title));
            let failed = match pr.checks {
                CheckRollup::Failing { failed } if pr.state.is_active() => failed,
                _ => 0,
            };
            let fix = (failed > 0 && !pr.failed.is_empty()).then(|| {
                let workspace = snap.workspace.clone();
                crate::ui::button_compact_sized(
                    "pr-fix-checks",
                    s::pr_fix_checks_btn(failed),
                    metrics,
                )
                .loading(snap.pr_sending)
                .disabled(snap.pr_sending)
                .on_click(cx.listener(move |_dock, _: &ClickEvent, window, cx| {
                    if let Some(ws) = workspace.upgrade() {
                        ws.update(cx, |ws, cx| ws.send_failed_checks(window, cx));
                    }
                }))
            });
            let line = row
                .when_some(glyph, |d, (path, color)| {
                    d.child(
                        crate::ui::icons::icon(path)
                            .with_size(px(metrics.icon(theme::GIT_BADGE_ARROW_SIZE)))
                            .text_color(color),
                    )
                })
                .child(title)
                .when_some(state_label(pr.state), |d, label| {
                    d.child(div().flex_none().text_color(subtle).child(label))
                })
                .when_some(
                    checks_label(pr.checks).filter(|_| pr.state.is_active()),
                    |d, label| d.child(div().flex_none().text_color(subtle).child(label)),
                )
                .child(open);
            let actions = fix.map(|fix| {
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .px(px(metrics.space(theme::GIT_HEADER_PAD_X)))
                    .pb(px(metrics.space(theme::GIT_HEADER_PAD_Y)))
                    .child(fix)
            });
            Some(
                div()
                    .flex()
                    .flex_col()
                    .child(line)
                    .when_some(actions, |d, actions| d.child(actions))
                    .into_any_element(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_counted_checks_get_a_label() {
        assert!(checks_label(CheckRollup::None).is_none());
        assert!(checks_label(CheckRollup::Pending).is_some());
        assert!(state_label(PrState::Open).is_none());
        assert!(state_label(PrState::Merged).is_some());
    }
}
