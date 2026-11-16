//! Flow definitions and execution history, scoped independently of the terminal.

use crate::ui::theme;
use crate::workspace::flow_browser::{FlowPageSnapshot, FlowTab};
use gpui::{AnyElement, IntoElement, prelude::*, px};

mod controls;
pub(in crate::workspace) mod detail;
pub(in crate::workspace) mod detail_render;
pub(in crate::workspace) mod execution_detail;
mod execution_evidence;
#[cfg(any(test, feature = "screenshot"))]
pub(in crate::workspace) mod execution_fixture;
mod execution_layout;
mod execution_render;
mod execution_selection;
#[cfg(feature = "screenshot")]
pub(in crate::workspace) mod execution_shots;
mod files;
pub(in crate::workspace) mod graph;
mod graph_menu;
mod graph_ops;
mod list;
mod live;
mod node_ops;
mod past;
mod rows;
mod run_detail;
mod run_report;
mod session_ops;
pub(in crate::workspace) mod start_navigation;
mod toolbar;

pub(in crate::workspace) use controls::header;

pub(in crate::workspace) fn render(snap: &FlowPageSnapshot, cx: &gpui::App) -> AnyElement {
    let state = &snap.flow_browser.state;
    let runs: Vec<_> = snap
        .groups
        .iter()
        .map(|group| {
            list::RunList::project(
                &group.flows,
                group.history.as_ref(),
                state.run_filter,
                &snap.flow_browser.query,
            )
        })
        .collect();
    let files: Vec<_> = snap
        .groups
        .iter()
        .map(|group| {
            list::DefinitionList::project(&group.files, state.origin, &snap.flow_browser.query)
        })
        .collect();
    let distinct_files = list::DefinitionList::distinct(
        snap.groups.iter().map(|group| group.files.as_slice()),
        state.origin,
        &snap.flow_browser.query,
    );
    let total_runs = runs.iter().map(|list| list.total).sum();
    let total_files = distinct_files.total;
    let waiting = runs.iter().map(|list| list.waiting).sum();
    let mut body = gpui::div()
        .flex()
        .flex_col()
        .gap(px(theme::RIGHT_PANEL_SECTION_GAP))
        .text_size(px(theme::RIGHT_PANEL_BODY_FONT_SIZE));
    if waiting > 0 {
        body = body.child(toolbar::attention(snap, waiting, cx));
    }
    let (visible, total) = match state.tab {
        FlowTab::Definitions => {
            body = body.child(controls::origin_tabs(snap, distinct_files.counts));
            (distinct_files.visible.len(), total_files)
        }
        FlowTab::Runs => {
            let counts = std::array::from_fn(|i| runs.iter().map(|list| list.counts[i]).sum());
            body = body.child(controls::status_tabs(snap, counts));
            (
                runs.iter().map(|list| list.visible.len()).sum::<usize>(),
                total_runs,
            )
        }
    };
    body = body
        .child(toolbar::search(snap, total_files, total_runs, cx))
        .child(toolbar::results(snap, visible, total, cx));
    if visible == 0 {
        body = body.child(toolbar::empty(snap, total, cx));
    } else {
        for (index, group) in snap.groups.iter().enumerate() {
            let count = match state.tab {
                FlowTab::Definitions => files[index].visible.len(),
                FlowTab::Runs => runs[index].visible.len(),
            };
            if count == 0 {
                continue;
            }
            let table = match state.tab {
                FlowTab::Definitions => rows::definitions(snap, group, &files[index], cx),
                FlowTab::Runs => rows::runs(snap, group, &runs[index], cx),
            };
            body = body.child(
                gpui::div()
                    .id(gpui::SharedString::from(format!(
                        "flow-worktree-{}-{}",
                        group.lane.project, group.lane.lane
                    )))
                    .when(snap.groups.len() > 1, |group_body| {
                        group_body.child(
                            gpui::div()
                                .py(px(theme::PAD_SM))
                                .text_color(theme::current(cx).text_muted)
                                .child(crate::surface::strings::common::filter_count(
                                    &group.label,
                                    count,
                                )),
                        )
                    })
                    .child(table),
            );
        }
    }
    if state.tab == FlowTab::Runs && total_runs > 0 {
        body = body.child(past::retention_note(cx));
    }
    body.into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn definitions_and_runs_are_separate_views() {
        assert_ne!(super::FlowTab::Definitions, super::FlowTab::Runs);
    }
}
