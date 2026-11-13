//! Draws the Pull requests page's detail in place of its list, from the
//! view staged for this frame.

use daruda_github::CheckStatus;
use gpui::{AnyElement, Context, Div, FontWeight, Hsla, SharedString, div, prelude::*, px};

use super::detail::{Load, PrDetailId, PrTab};
use super::detail_view::{
    ActionSlot, CheckRowView, DetailBody, Fact, HeaderActions, LogSlot, MergeView, Mood,
    PrDetailView, ReadyView, SendSlot, SideView, ThreadRowView, ThreadsView,
};
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{
    Badge, Button, Disableable as _, DropdownMenu as _, PopupMenuItem, Selectable as _,
    Sizable as _, button, button_compact, button_group, button_icon, button_primary,
    button_with_icon, icons, menu_builder, theme,
};
use crate::workspace::Workspace;

pub(in crate::workspace) fn render(view: &PrDetailView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let body = div()
        .mx_auto()
        .w_full()
        .max_w(px(theme::DOCK_PAGE_MAX_WIDTH))
        .p(px(theme::DOCK_PAGE_PAD))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_LG))
        .child(title_block(view, cx))
        .child(tabs(view, cx))
        .child(match &view.body {
            DetailBody::Loading => note(common::loading(), cx).into_any_element(),
            DetailBody::Failed(reason) => note(s::detail_failed(reason), cx).into_any_element(),
            DetailBody::Ready(ready) => div()
                .flex()
                .items_start()
                .gap(px(theme::DOCK_PAGE_PAD))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(tab_content(view.id, view.tab, ready, cx)),
                )
                .child(side(&ready.side, cx))
                .into_any_element(),
        });
    div()
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(t.dock_bg)
        .child(crumb_bar(view, cx))
        .child(
            div()
                .id(("pr-detail-scroll", view.id.0 as usize))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

/// Back to the list, where this is, and refresh.
fn crumb_bar(view: &PrDetailView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let id = view.id;
    div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_LG))
        .h(px(theme::TASK_EDIT_HEADER_H))
        .px(px(theme::PAD_LG))
        .border_b_1()
        .border_color(t.border)
        .child(
            button_icon("pr-detail-back", icons::BACK, cx)
                .tooltip(s::back_to_list())
                .debug_selector(|| "pr-detail-back".into())
                .on_click(cx.listener(|ws, _, window, cx| ws.back_to_pr_list(window, cx))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(view.crumb.clone()),
        )
        .child(
            button_icon("pr-detail-refresh", icons::REFRESH, cx)
                .tooltip(common::refresh())
                .on_click(cx.listener(move |ws, _, _, cx| ws.refresh_pr_detail(id, cx))),
        )
        .into_any_element()
}

/// State, number and title, the sub-line, and the actions on the right.
fn title_block(view: &PrDetailView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let id = view.id;
    let lane = view.lane.as_ref().map(|lane| {
        button_with_icon(
            "pr-detail-lane",
            s::open_in_lane(&lane.name),
            icons::FOLDER_OPEN,
        )
        .debug_selector(|| "pr-detail-lane".into())
        .on_click(cx.listener(move |ws, _, window, cx| ws.open_pr_detail_lane(id, window, cx)))
    });
    let github = view.url.clone().map(|url| {
        button_with_icon("pr-detail-github", s::github(), icons::OPEN_IN_NEW)
            .tooltip(crate::surface::strings::git::pr_open_on_github())
            .on_click(move |_, _, cx| cx.open_url(&url))
    });
    div()
        .flex()
        .items_start()
        .gap(px(theme::GAP_LG))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_SM))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(theme::GAP_LG))
                        .min_w_0()
                        .child(
                            Badge::new(view.state.clone())
                                .text_color(mood_color(view.state_mood, cx)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(theme::FONT_SIZE_LG))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .child(view.title.clone()),
                        ),
                )
                .child(
                    div()
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_subtle)
                        .child(view.meta.clone()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .gap(px(theme::GAP_SM))
                .children(lane)
                .children(github)
                .children(
                    view.actions
                        .as_ref()
                        .map(|actions| header_actions(id, actions, cx)),
                ),
        )
        .into_any_element()
}

/// An action button as its slot says: spinning while it runs, off while
/// another runs, and saying why when it cannot run.
fn with_slot(button: Button, slot: &ActionSlot) -> Button {
    button
        .loading(*slot == ActionSlot::Running)
        .disabled(!slot.enabled())
        .when_some(slot.reason(), |button, reason| button.tooltip(reason))
}

/// Ready for review on a draft, Close, and the merge button.
fn header_actions(id: PrDetailId, actions: &HeaderActions, cx: &Context<Workspace>) -> AnyElement {
    let ready = actions.ready.as_ref().map(|slot| {
        with_slot(button("pr-detail-ready", s::action_ready()), slot)
            .debug_selector(|| "pr-detail-ready".into())
            .on_click(cx.listener(move |ws, _, _, cx| ws.mark_pr_detail_ready(id, cx)))
    });
    div()
        .flex()
        .gap(px(theme::GAP_SM))
        .children(ready)
        .child(
            with_slot(button("pr-detail-close", s::action_close()), &actions.close)
                .debug_selector(|| "pr-detail-close".into())
                .on_click(cx.listener(move |ws, _, window, cx| ws.ask_pr_close(id, window, cx))),
        )
        .child(merge_button(id, &actions.merge, cx))
        .into_any_element()
}

/// The preferred method's label, opening a menu of the allowed methods;
/// each asks before it merges. Without a menu while it cannot merge.
fn merge_button(id: PrDetailId, merge: &MergeView, cx: &Context<Workspace>) -> AnyElement {
    let button = with_slot(
        button_primary("pr-detail-merge", merge.label.clone()),
        &merge.slot,
    )
    .debug_selector(|| "pr-detail-merge".into())
    .child(icons::icon(icons::EXPAND_MORE));
    if !merge.slot.enabled() {
        return button.into_any_element();
    }
    let methods = merge.methods.clone();
    let workspace = cx.weak_entity();
    button
        .dropdown_menu_with_anchor(
            gpui::Anchor::TopRight,
            menu_builder(move |menu, _, _| {
                methods
                    .iter()
                    .cloned()
                    .fold(menu, |menu, (method, label)| {
                        let workspace = workspace.clone();
                        menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                            if let Some(ws) = workspace.upgrade() {
                                ws.update(cx, |ws, cx| ws.ask_pr_merge(id, method, window, cx));
                            }
                        }))
                    })
                    .separator()
                    .label(s::merge_menu_note())
            }),
        )
        .into_any_element()
}

fn tabs(view: &PrDetailView, cx: &Context<Workspace>) -> AnyElement {
    let id = view.id;
    button_group("pr-detail-tabs", cx)
        .children(view.tabs.iter().enumerate().map(|(ix, (tab, label))| {
            button(("pr-detail-tab", ix), label.clone())
                .debug_selector(move || format!("pr-detail-tab-{ix}"))
                .selected(*tab == view.tab)
                .tab_stop(true)
        }))
        .on_click(cx.listener(move |ws, selected: &Vec<usize>, _, cx| {
            if let Some(tab) = selected.first().and_then(|ix| PrTab::ALL.get(*ix)) {
                ws.select_pr_detail_tab(id, *tab, cx);
            }
        }))
        .into_any_element()
}

fn tab_content(
    id: PrDetailId,
    tab: PrTab,
    ready: &ReadyView,
    cx: &Context<Workspace>,
) -> AnyElement {
    match tab {
        PrTab::Checks => checks(id, ready, cx),
        PrTab::Threads => threads(id, ready, cx),
        PrTab::Description => match &ready.description {
            Some(body) => {
                crate::ui::markdown(("pr-detail-description", id.0 as usize), body.clone())
                    .selectable(true)
                    .full_width(true)
                    .into_any_element()
            }
            None => note(s::description_empty(), cx).into_any_element(),
        },
    }
}

/// A line above a tab's list: what it holds, and Send to agent.
fn send_line(
    summary: Option<String>,
    send: SendSlot,
    nothing: String,
    on_send: impl Fn(&mut Workspace, &mut gpui::Window, &mut Context<Workspace>) + 'static,
    selector: &'static str,
    extra: Option<AnyElement>,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let tooltip = match send {
        SendSlot::NoLane => Some(s::send_no_lane()),
        SendSlot::Nothing => Some(nothing),
        SendSlot::Sending | SendSlot::Ready => None,
    };
    div()
        .flex()
        .items_center()
        .gap(px(theme::GAP_LG))
        .pb(px(theme::PAD_SM))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .children(summary),
        )
        .children(extra)
        .child(
            button_compact(selector, s::send_to_agent())
                .debug_selector(move || selector.into())
                .loading(send == SendSlot::Sending)
                .disabled(!send.enabled())
                .when_some(tooltip, |button, tooltip| button.tooltip(tooltip))
                .on_click(cx.listener(move |ws, _, window, cx| on_send(ws, window, cx))),
        )
        .into_any_element()
}

fn checks(id: PrDetailId, ready: &ReadyView, cx: &Context<Workspace>) -> AnyElement {
    let rows: Vec<AnyElement> = ready
        .checks
        .iter()
        .enumerate()
        .map(|(ix, row)| check_row(id, ix, row, cx))
        .collect();
    div()
        .flex()
        .flex_col()
        .child(send_line(
            Some(ready.checks_summary.clone()),
            ready.checks_send,
            s::send_nothing_checks(),
            move |ws, window, cx| ws.send_pr_detail_checks(id, window, cx),
            "pr-detail-send-checks",
            ready.rerun.as_ref().map(|slot| {
                with_slot(button_compact("pr-detail-rerun", s::action_rerun()), slot)
                    .debug_selector(|| "pr-detail-rerun".into())
                    .on_click(cx.listener(move |ws, _, _, cx| ws.rerun_pr_detail_checks(id, cx)))
                    .into_any_element()
            }),
            cx,
        ))
        .children(rows)
        .into_any_element()
}

fn check_row(id: PrDetailId, ix: usize, row: &CheckRowView, cx: &Context<Workspace>) -> AnyElement {
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
    div()
        .flex()
        .flex_col()
        .child(line)
        .children(log)
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

fn threads(id: PrDetailId, ready: &ReadyView, cx: &Context<Workspace>) -> AnyElement {
    let send = send_line(
        None,
        ready.threads_send,
        s::send_nothing_threads(),
        move |ws, window, cx| ws.send_pr_detail_threads(id, window, cx),
        "pr-detail-send-threads",
        None,
        cx,
    );
    let body: Vec<AnyElement> = match &ready.threads {
        ThreadsView::Loading => vec![note(common::loading(), cx).into_any_element()],
        ThreadsView::Failed(reason) => {
            vec![note(s::threads_failed(reason), cx).into_any_element()]
        }
        ThreadsView::Listed { rows, truncated } => {
            let mut body: Vec<AnyElement> = rows.iter().map(|row| thread_row(row, cx)).collect();
            if rows.is_empty() {
                body.push(note(s::threads_empty(), cx).into_any_element());
            }
            if *truncated {
                body.push(note(s::threads_truncated(), cx).into_any_element());
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

fn thread_row(row: &ThreadRowView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_XS))
        .px(px(theme::LIST_ROW_PAD_X))
        .py(px(theme::PAD_SM))
        .rounded(px(theme::LIST_ROW_RADIUS))
        .hover(|style| style.bg(t.overlay_hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_LG))
                .min_w_0()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_body)
                        .child(row.location.clone()),
                )
                .when(row.outdated, |line| {
                    line.child(Badge::new(s::thread_outdated()).text_color(t.text_muted))
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
        .child(
            div()
                .flex()
                .gap(px(theme::GAP_SM))
                .min_w_0()
                .text_size(px(theme::FONT_SIZE_SM))
                .child(
                    div()
                        .flex_none()
                        .text_color(t.text_muted)
                        .child(row.author.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_subtle)
                        .child(row.excerpt.clone()),
                ),
        )
        .into_any_element()
}

/// Merge, reviews and the worktree.
fn side(view: &SideView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let lane = match &view.lane {
        Some(lane) => fact_value(lane.name.clone(), t.text_body),
        None => fact_value(s::lane_none(), t.text_subtle),
    };
    div()
        .flex_none()
        .w(px(theme::PR_DETAIL_SIDE_W))
        .flex()
        .flex_col()
        .gap(px(theme::PAD_XL))
        .child(section(s::side_merge(), facts(&view.merge, cx), cx))
        .child(section(s::side_reviews(), facts(&view.reviews, cx), cx))
        .child(section(
            s::side_lane(),
            div()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_SM))
                .child(lane)
                .children(view.lane_note.clone().map(|note| {
                    div()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.banner_warning_text)
                        .child(note)
                }))
                .into_any_element(),
            cx,
        ))
        .into_any_element()
}

fn section(title: String, body: AnyElement, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_STANDARD))
        .pb(px(theme::PAD_LG))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text_muted)
                .child(title),
        )
        .child(body)
        .into_any_element()
}

fn facts(facts: &[Fact], cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_SM))
        .children(facts.iter().map(|fact| {
            div()
                .flex()
                .justify_between()
                .gap(px(theme::GAP_LG))
                .text_size(px(theme::FONT_SIZE_SM))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(t.text_subtle)
                        .child(fact.label.clone()),
                )
                .child(fact_value(fact.value.clone(), mood_color(fact.mood, cx)))
        }))
        .into_any_element()
}

fn fact_value(value: String, color: Hsla) -> Div {
    div()
        .min_w_0()
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(color)
        .child(value)
}

/// A quiet line in place of content.
fn note(message: impl Into<SharedString>, cx: &Context<Workspace>) -> Div {
    div()
        .py(px(theme::PAD_SM))
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(theme::current(cx).text_subtle)
        .child(message.into())
}

fn mood_color(mood: Mood, cx: &Context<Workspace>) -> Hsla {
    let t = theme::current(cx);
    match mood {
        Mood::Good => t.banner_success_text,
        Mood::Warn => t.banner_warning_text,
        Mood::Bad => t.banner_error_text,
        Mood::Quiet => t.text_muted,
    }
}

/// The glyph a check's status shows, the same family the list's tone uses.
fn status_glyph(status: CheckStatus, cx: &Context<Workspace>) -> (&'static str, Hsla) {
    let t = theme::current(cx);
    match status {
        CheckStatus::Failure => (icons::WARNING, t.banner_error_text),
        CheckStatus::Pending => (icons::RADIO_OFF, t.banner_warning_text),
        CheckStatus::Success => (icons::CHECK_CIRCLE, t.banner_success_text),
        CheckStatus::Ignored => (icons::RADIO_OFF, t.text_subtle),
    }
}
