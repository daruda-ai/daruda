//! Page identity, worktree scope and counted tabs share the Tasks chrome.

use super::list::ORIGINS;
use crate::surface::strings as s;
use crate::ui::{
    ButtonVariants as _, Disableable as _, DropdownMenu as _, PopupMenuItem, button_bare,
    button_with_icon, icons, list_page, menu_builder, tab, tab_bar, theme,
};
use crate::workspace::{
    flow_browser::FlowPageSnapshot,
    flow_browser::{FlowScope, FlowTab, RunFilter},
    flow_paths::FlowOrigin,
};
use gpui::{AnyElement, App, IntoElement, div, prelude::*, px};

pub(in crate::workspace) fn header(
    snap: &FlowPageSnapshot,
    close: AnyElement,
    cx: &App,
) -> AnyElement {
    let scope = div()
        .flex()
        .items_center()
        .min_w_0()
        .gap(px(theme::GAP_SM))
        .child(crate::workspace::pages::chrome::project_picker(
            &snap.scope,
            snap.workspace.clone(),
            cx,
        ))
        .child(scope_picker(snap, cx));
    list_page::header(
        s::dock::right_tab_flows(),
        scope,
        new_button(snap),
        close,
        cx,
    )
    .into_any_element()
}

fn new_button(snap: &FlowPageSnapshot) -> AnyElement {
    let targets: Vec<_> = snap
        .flow_browser
        .targets
        .iter()
        .filter(|target| snap.groups.iter().any(|group| group.lane == target.lane))
        .cloned()
        .collect();
    let workspace = snap.workspace.clone();
    let button = button_with_icon("flow-new", s::flow::new_title(), icons::ADD)
        .primary()
        .flex_none()
        .tab_stop(true)
        .disabled(targets.is_empty());
    if targets.len() == 1 {
        let lane = targets[0].lane;
        button
            .tooltip(s::flow::new_in_project(&targets[0].project))
            .on_click(move |_, window, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.prompt_new_flow(lane, window, cx));
                }
            })
            .into_any_element()
    } else {
        button
            .tooltip(s::common::choose_creation_project())
            .dropdown_menu(menu_builder(move |menu, _, _| {
                targets.iter().fold(menu.scrollable(true), |menu, target| {
                    let workspace = workspace.clone();
                    let lane = target.lane;
                    menu.item(PopupMenuItem::new(target.label.clone()).on_click(
                        move |_, window, cx| {
                            if let Some(ws) = workspace.upgrade() {
                                ws.update(cx, |ws, cx| ws.prompt_new_flow(lane, window, cx));
                            }
                        },
                    ))
                })
            }))
            .into_any_element()
    }
}

fn scope_picker(snap: &FlowPageSnapshot, cx: &App) -> impl IntoElement {
    let targets = snap.flow_browser.targets.clone();
    let selected = snap.flow_browser.state.scope;
    let workspace = snap.workspace.clone();
    let label = match selected {
        FlowScope::All => s::flow::all_worktrees(),
        FlowScope::Worktree(lane) => targets
            .iter()
            .find(|target| target.lane == lane)
            .map(|target| target.label.clone())
            .unwrap_or_else(s::flow::scope_unavailable),
    };
    button_bare("flow-scope")
        .debug_selector(|| "flow-scope".into())
        .tab_stop(true)
        .min_w_0()
        .tooltip(s::flow::scope_hint())
        .child(icons::icon(icons::FOLDER).text_color(theme::current(cx).text_muted))
        .child(
            div()
                .min_w_0()
                .max_w(px(theme::list_metrics::WORKTREE_SCOPE_MAX_W))
                .truncate()
                .child(label),
        )
        .child(icons::icon(icons::EXPAND_MORE))
        .dropdown_menu(menu_builder(move |menu, _, _| {
            let all_ws = workspace.clone();
            let menu = menu.scrollable(true).item(
                PopupMenuItem::new(s::flow::all_worktrees())
                    .checked(selected == FlowScope::All)
                    .on_click(move |_, _, cx| {
                        if let Some(ws) = all_ws.upgrade() {
                            ws.update(cx, |ws, cx| ws.set_flow_scope(FlowScope::All, cx));
                        }
                    }),
            );
            targets.iter().fold(menu, |menu, target| {
                let workspace = workspace.clone();
                let scope = FlowScope::Worktree(target.lane);
                menu.item(
                    PopupMenuItem::new(target.label.clone())
                        .checked(selected == scope)
                        .on_click(move |_, _, cx| {
                            if let Some(ws) = workspace.upgrade() {
                                ws.update(cx, |ws, cx| ws.set_flow_scope(scope, cx));
                            }
                        }),
                )
            })
        }))
}

pub(super) fn section_tabs(
    snap: &FlowPageSnapshot,
    file_count: usize,
    run_count: usize,
) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    tab_bar("flow-section-tabs")
        .flex_none()
        .selected_index(snap.flow_browser.state.tab.index())
        .child(
            tab(s::common::filter_count(
                s::flow::panel_flows_heading(),
                file_count,
            ))
            .debug_selector(|| "flow-tab-definitions".into()),
        )
        .child(
            tab(s::common::filter_count(s::flow::runs_tab(), run_count))
                .debug_selector(|| "flow-tab-runs".into()),
        )
        .on_click(move |index, _, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| {
                    ws.set_flow_tab(
                        if *index == 0 {
                            FlowTab::Definitions
                        } else {
                            FlowTab::Runs
                        },
                        cx,
                    )
                });
            }
        })
}

pub(super) fn origin_label(origin: Option<FlowOrigin>) -> String {
    match origin {
        None => s::flow::all_sources(),
        Some(FlowOrigin::Repo) => s::flow::source_repo(),
        Some(FlowOrigin::Project) => s::flow::source_project(),
        Some(FlowOrigin::Global) => s::flow::source_global(),
    }
}

pub(super) fn run_label(filter: RunFilter) -> String {
    match filter {
        RunFilter::All => s::flow::all_runs(),
        RunFilter::Running => s::flow::status_running(),
        RunFilter::Asking => s::flow::filter_asking(),
        RunFilter::Done => s::flow::status_done(),
        RunFilter::Failed => s::flow::status_failed(),
        RunFilter::Resumable => s::flow::filter_resumable(),
        RunFilter::Canceled => s::flow::status_canceled(),
        RunFilter::Unknown => s::flow::status_unknown(),
    }
}

pub(super) fn origin_tabs(snap: &FlowPageSnapshot, counts: [usize; 4]) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    tab_bar("flow-origin-tabs")
        .w_full()
        .menu(true)
        .selected_index(
            ORIGINS
                .iter()
                .position(|origin| *origin == snap.flow_browser.state.origin)
                .unwrap_or(0),
        )
        .children(
            ORIGINS
                .into_iter()
                .zip(counts)
                .enumerate()
                .map(|(index, (origin, count))| {
                    tab(s::common::filter_count(origin_label(origin), count))
                        .debug_selector(move || format!("flow-origin-{index}"))
                }),
        )
        .on_click(move |index, _, cx| {
            if let Some(origin) = ORIGINS.get(*index).copied()
                && let Some(ws) = workspace.upgrade()
            {
                ws.update(cx, |ws, cx| ws.set_flow_origin(origin, cx));
            }
        })
}

pub(super) fn status_tabs(snap: &FlowPageSnapshot, counts: [usize; 8]) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    tab_bar("flow-status-tabs")
        .w_full()
        .menu(true)
        .selected_index(
            RunFilter::ALL
                .iter()
                .position(|filter| *filter == snap.flow_browser.state.run_filter)
                .unwrap_or(0),
        )
        .children(RunFilter::ALL.into_iter().zip(counts).enumerate().map(
            |(index, (filter, count))| {
                tab(s::common::filter_count(run_label(filter), count))
                    .debug_selector(move || format!("flow-status-{index}"))
            },
        ))
        .on_click(move |index, _, cx| {
            if let Some(filter) = RunFilter::ALL.get(*index).copied()
                && let Some(ws) = workspace.upgrade()
            {
                ws.update(cx, |ws, cx| ws.set_flow_run_filter(filter, cx));
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sources_and_execution_statuses_have_distinct_labels() {
        assert_ne!(
            origin_label(Some(FlowOrigin::Repo)),
            origin_label(Some(FlowOrigin::Project))
        );
        assert_ne!(
            run_label(RunFilter::Failed),
            run_label(RunFilter::Resumable)
        );
    }
}
