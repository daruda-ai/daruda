//! Draws the Pull requests page's detail in place of its list, from the
//! view staged for this frame.

use gpui::{
    AnyElement, Context, Div, FontWeight, Hsla, SharedString, div, prelude::*, px, relative,
};

use super::super::list::LaneSlot;
use super::view::{
    ActionNotice, ActionSlot, DetailBody, Fact, HeaderActions, HeaderControls, LaneNote, MergeView,
    Mood, PrDetailView, ReadyView, SendSlot, SideView,
};
use super::{PrDetailId, PrTab};
use crate::surface::strings::{common, git as git_s, pull_requests as s};
use crate::ui::scrollbar::{Scrollbar, ScrollbarMode};
use crate::ui::{
    Badge, Button, Disableable as _, DropdownMenu as _, PopupMenuItem, Selectable as _, button,
    button_compact, button_group, button_primary, button_with_icon, icons, menu_builder, theme,
};
use crate::workspace::Workspace;

// Keep some space for the source even when the prompt editor is open.
const HANDOFF_PANEL_MAX_HEIGHT: f32 = 0.65;

pub(in crate::workspace) fn render(
    view: &PrDetailView,
    width: f32,
    scroll: &gpui::ScrollHandle,
    files_scroll: &gpui::UniformListScrollHandle,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let body = div()
        .w_full()
        .min_w_0()
        .p(px(theme::DOCK_PAGE_PAD))
        .flex()
        .flex_col()
        .gap(px(theme::GAP_LG))
        .children(view.missing_comment_url.clone().map(|url| {
            div()
                .flex()
                .items_center()
                .gap(px(theme::GAP_SM))
                .child(note(s::linked_comment_unavailable(), cx))
                .child(
                    button_with_icon(
                        "pr-linked-comment-github",
                        git_s::pr_open_on_github(),
                        icons::OPEN_IN_NEW,
                    )
                    .debug_selector(|| "pr-linked-comment-github".into())
                    .on_click(move |_, _, cx| cx.open_url(&url)),
                )
        }))
        .children(view.notice.clone().map(|notice| action_notice(notice, cx)))
        .child(match &view.body {
            DetailBody::Loading => note(common::loading(), cx).into_any_element(),
            DetailBody::Failed(reason) => note(s::detail_failed(reason), cx).into_any_element(),
            DetailBody::Ready(ready) => div()
                .flex()
                .flex_wrap()
                .items_start()
                .gap(px(theme::DOCK_PAGE_PAD))
                .child(
                    div()
                        .debug_selector(|| "pr-detail-tab-content".into())
                        .flex_1()
                        .flex_basis(px(theme::PR_DETAIL_CONTENT_BASIS))
                        .min_w_0()
                        .child(tab_content(view, ready, files_scroll, width, cx)),
                )
                .when(view.tab != PrTab::Files, |body| {
                    body.child(side(view.id, &ready.side, cx))
                })
                .into_any_element(),
        });
    div()
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .bg(t.dock_bg)
        .child(
            div()
                .debug_selector(|| "pr-detail-controls".into())
                .flex()
                .flex_col()
                .flex_none()
                .min_w_0()
                .p(px(theme::DOCK_PAGE_PAD))
                .gap(px(theme::GAP_LG))
                .border_b_1()
                .border_color(t.border)
                .child(title_block(view, cx))
                .child(tabs(view, cx)),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .children(view.handoff.as_ref().map(|handoff| {
                    div()
                        .id(("pr-handoff-scroll", view.id.0 as usize))
                        .flex_none()
                        .w_full()
                        .max_h(relative(HANDOFF_PANEL_MAX_HEIGHT))
                        .overflow_y_scroll()
                        .p(px(theme::DOCK_PAGE_PAD))
                        .child(super::handoff_render::handoff(view.id, handoff, cx))
                }))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id(("pr-detail-scroll", view.id.0 as usize))
                                .track_scroll(scroll)
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                // A sideways swipe over a wide diff scrolls the diff.
                                .restrict_scroll_to_axis()
                                .child(body),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .debug_selector(|| "pr-detail-scrollbar".into())
                                .child(
                                    Scrollbar::vertical(scroll)
                                        .id(("pr-detail-scrollbar", view.id.0 as usize))
                                        .mode(ScrollbarMode::Always),
                                ),
                        ),
                ),
        )
        .children(match &view.body {
            DetailBody::Ready(ready) => Some({
                let id = view.id;
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(t.border)
                    .px(px(theme::DOCK_PAGE_PAD))
                    .py(px(theme::PAD_SM))
                    .gap(px(theme::GAP_SM))
                    .text_size(px(theme::FONT_SIZE_SM))
                    .text_color(t.text_muted)
                    .child(ready.review_footer.clone())
                    .child(
                        button_compact("pr-review-open", s::review_open())
                            .debug_selector(|| "pr-review-open".into())
                            .on_click(cx.listener(move |ws, _, window, cx| {
                                ws.show_pr_review(id, window, cx)
                            })),
                    )
            }),
            _ => None,
        })
        .into_any_element()
}

/// State, number and title, the sub-line, and the actions on the right.
fn title_block(view: &PrDetailView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let id = view.id;
    let lane = lane_button(view, cx);
    let github = view.url.clone().map(|url| {
        button_with_icon("pr-detail-github", s::github(), icons::OPEN_IN_NEW)
            .tooltip(crate::surface::strings::git::pr_open_on_github())
            .on_click(move |_, _, cx| cx.open_url(&url))
    });
    div()
        .flex()
        .flex_col()
        .w_full()
        .items_start()
        .gap(px(theme::GAP_LG))
        .child(
            div()
                .w_full()
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
                                .id(("pr-detail-title", view.id.0 as usize))
                                .debug_selector(|| "pr-detail-title".into())
                                .min_w_0()
                                .truncate()
                                .tooltip(crate::ui::tooltip::text(view.title.clone()))
                                .text_size(px(theme::FONT_SIZE_LG))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.text_primary)
                                .child(view.title.clone()),
                        ),
                )
                .child(
                    div()
                        .id(("pr-detail-meta", view.id.0 as usize))
                        .w_full()
                        .truncate()
                        .tooltip(crate::ui::tooltip::text(view.meta.clone()))
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(t.text_subtle)
                        .child(view.meta.clone()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .w_full()
                .flex_none()
                .gap(px(theme::GAP_SM))
                .child(lane)
                .children(github)
                .children(match &view.body {
                    DetailBody::Ready(ready) => ready.inspect_failures.as_ref().map(|label| {
                        button_compact("pr-inspect-failures", label.clone()).on_click(
                            cx.listener(move |ws, _, _, cx| ws.inspect_failed_pr_checks(id, cx)),
                        )
                    }),
                    _ => None,
                })
                .children(match &view.header {
                    HeaderControls::None => None,
                    HeaderControls::Active(actions) => Some(header_actions(id, actions, cx)),
                    HeaderControls::Reopen(slot) => Some(
                        with_slot(button("pr-detail-reopen", s::action_reopen()), slot)
                            .debug_selector(|| "pr-detail-reopen".into())
                            .on_click(cx.listener(move |ws, _, _, cx| ws.reopen_pr_detail(id, cx)))
                            .into_any_element(),
                    ),
                }),
        )
        .into_any_element()
}

/// The worktree working on the pull request, or the list row's button that
/// makes one, inert while it is being made.
fn lane_button(view: &PrDetailView, cx: &Context<Workspace>) -> AnyElement {
    let id = view.id;
    let (creating, review) = match &view.lane {
        LaneSlot::Linked(lane) => {
            return button_with_icon(
                "pr-detail-lane",
                s::open_in_lane(&lane.name),
                icons::FOLDER_OPEN,
            )
            .max_w_full()
            .debug_selector(|| "pr-detail-lane".into())
            .on_click(cx.listener(move |ws, _, window, cx| ws.open_pr_detail_lane(id, window, cx)))
            .into_any_element();
        }
        LaneSlot::Creating { review } => (true, *review),
        LaneSlot::Absent { review } => (false, *review),
    };
    use super::super::CheckoutStage;
    let label = match (creating, view.checkout_stage) {
        (true, Some(CheckoutStage::Fetching)) => s::checkout_fetching(),
        (true, Some(CheckoutStage::Creating)) => s::checkout_creating(),
        _ => super::super::list::create_lane_label(review),
    };
    button("pr-detail-create-lane", label)
        .debug_selector(|| "pr-detail-create-lane".into())
        .loading(creating)
        .disabled(creating)
        .on_click(cx.listener(move |ws, _, window, cx| ws.create_pr_detail_lane(id, window, cx)))
        .into_any_element()
}

/// What the last action's failure said, as `gh` put it.
fn action_notice(notice: ActionNotice, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let (line, color, selector) = match notice {
        ActionNotice::Failed(line) => (line, t.banner_error_text, "pr-detail-error"),
        ActionNotice::Done(line) => (line, t.text_muted, "pr-detail-done"),
    };
    div()
        .debug_selector(move || selector.into())
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(color)
        .child(line)
        .into_any_element()
}

/// An action button as its slot says: spinning while it runs, off while
/// another runs, and saying why when it cannot run.
pub(super) fn with_slot(button: Button, slot: &ActionSlot) -> Button {
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
        .flex_none()
        .gap(px(theme::GAP_SM))
        .children(ready)
        .children(actions.auto_merge_off.as_ref().map(|slot| {
            with_slot(
                button("pr-detail-auto-merge-off", s::auto_merge_off()),
                slot,
            )
            .debug_selector(|| "pr-detail-auto-merge-off".into())
            .on_click(cx.listener(move |ws, _, _, cx| ws.disable_pr_auto_merge(id, cx)))
        }))
        .children(actions.dequeue.as_ref().map(|slot| {
            with_slot(button("pr-detail-dequeue", s::dequeue()), slot)
                .debug_selector(|| "pr-detail-dequeue".into())
                .on_click(cx.listener(move |ws, _, window, cx| ws.ask_pr_dequeue(id, window, cx)))
        }))
        .child(
            with_slot(button("pr-detail-close", s::action_close()), &actions.close)
                .debug_selector(|| "pr-detail-close".into())
                .on_click(cx.listener(move |ws, _, window, cx| ws.ask_pr_close(id, window, cx))),
        )
        .child(merge_button(id, &actions.merge, cx))
        .children(actions.merge.slot.reason().map(|reason| note(reason, cx)))
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
    let auto = merge.auto.clone();
    let auto_stack = merge.auto_stack.clone();
    let workspace = cx.weak_entity();
    button
        .dropdown_menu_with_anchor(
            gpui::Anchor::TopRight,
            menu_builder(move |menu, _, _| {
                let menu = methods.iter().cloned().fold(menu, |menu, (method, label)| {
                    let workspace = workspace.clone();
                    menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        if let Some(ws) = workspace.upgrade() {
                            ws.update(cx, |ws, cx| ws.ask_pr_merge(id, method, window, cx));
                        }
                    }))
                });
                // Merging later, once requirements pass: apart from now.
                let menu = if auto.is_empty() {
                    menu
                } else {
                    menu.separator()
                };
                let menu = auto.iter().cloned().fold(menu, |menu, (method, label)| {
                    let workspace = workspace.clone();
                    menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        if let Some(ws) = workspace.upgrade() {
                            ws.update(cx, |ws, cx| {
                                ws.ask_pr_auto_merge(id, method, None, window, cx)
                            });
                        }
                    }))
                });
                // With the stack below it: the range is named when asked.
                let menu = match &auto_stack {
                    Some((range, methods)) => {
                        methods.iter().cloned().fold(menu, |menu, (method, label)| {
                            let workspace = workspace.clone();
                            let range = range.clone();
                            menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                                if let Some(ws) = workspace.upgrade() {
                                    let range = Some(range.clone());
                                    ws.update(cx, |ws, cx| {
                                        ws.ask_pr_auto_merge(id, method, range, window, cx)
                                    });
                                }
                            }))
                        })
                    }
                    None => menu,
                };
                menu.separator().label(s::merge_menu_note())
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
    view: &PrDetailView,
    ready: &ReadyView,
    files_scroll: &gpui::UniformListScrollHandle,
    width: f32,
    cx: &Context<Workspace>,
) -> AnyElement {
    let id = view.id;
    match view.tab {
        PrTab::Checks => super::checks_render::checks(id, ready, cx),
        PrTab::Files => super::files_render::files(
            id,
            ready,
            view.url.as_deref(),
            files_scroll,
            width - 2.0 * theme::DOCK_PAGE_PAD >= theme::PR_FILES_SPLIT_MIN_W,
            cx,
        ),
        PrTab::Threads => super::threads_render::threads(id, ready, cx),
        PrTab::Description => super::description_render::description(id, ready, cx),
    }
}

/// A line above a tab's list: what it holds, and Send to agent.
pub(super) fn send_line(
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
        SendSlot::NotOpen => Some(s::send_not_open()),
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

/// Merge, reviews and the worktree.
fn side(id: PrDetailId, view: &SideView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    let lane = match &view.lane {
        Some(lane) => fact_value(lane.name.clone(), t.text_body),
        None => fact_value(s::lane_none(), t.text_subtle),
    };
    div()
        .debug_selector(|| "pr-detail-side".into())
        .flex_none()
        .w(px(theme::PR_DETAIL_SIDE_W))
        .max_w_full()
        .flex()
        .flex_col()
        .gap(px(theme::PAD_XL))
        .child(section(s::side_merge(), facts(&view.merge, cx), cx))
        .child(section(
            s::side_reviews(),
            div()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_LG))
                .child(facts(&view.reviews, cx))
                .children(
                    view.requests
                        .as_ref()
                        .map(|requests| super::reviewers_render::requests(id, requests, cx)),
                )
                .into_any_element(),
            cx,
        ))
        .children(view.stack.as_ref().map(|stack| {
            section(
                s::side_stack(),
                super::stack_render::stack(id, view.item.0.clone(), stack, cx),
                cx,
            )
        }))
        .child(section(
            s::side_lane(),
            div()
                .flex()
                .flex_col()
                .gap(px(theme::GAP_SM))
                .child(lane)
                .children(view.lane_note.map(|note| {
                    let (text, color) = match note {
                        LaneNote::UpdatesPr => (s::same_repo_lane_note(), t.text_subtle),
                        LaneNote::ReviewOnly => (s::fork_lane_note(), t.banner_warning_text),
                    };
                    div()
                        .text_size(px(theme::FONT_SIZE_SM))
                        .text_color(color)
                        .child(text)
                }))
                .child(super::links_render::links(view, cx))
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
pub(super) fn note(message: impl Into<SharedString>, cx: &Context<Workspace>) -> Div {
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

/// Under a review editor: why what is typed is not kept across restarts,
/// when it is not.
pub(super) fn keeping_note(
    keeping: crate::github::DraftKeeping,
    cx: &Context<Workspace>,
) -> Option<AnyElement> {
    use crate::github::DraftKeeping;
    let text = match keeping {
        DraftKeeping::Saved => return None,
        DraftKeeping::MemoryOnly => s::drafts_memory_only(),
        DraftKeeping::SaveFailed => s::drafts_save_failing(),
        DraftKeeping::NoAccount => s::drafts_no_account(),
    };
    Some(
        div()
            .text_size(px(theme::FONT_SIZE_SM))
            .text_color(theme::current(cx).banner_warning_text)
            .child(text)
            .into_any_element(),
    )
}
