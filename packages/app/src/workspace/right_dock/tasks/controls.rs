//! Task-page scope, status tabs, and recoverable empty states.

use daruda_store::tasks::{TaskFilter, TaskScope};
use gpui::{AnyElement, App, IntoElement, SharedString, prelude::*};

use super::list::{FILTERS, TaskList};
use super::{TaskGroupKey, TaskGrouping, grouping::TaskGroup};
use crate::surface::strings;
use crate::ui::{
    ButtonVariants as _, Disableable as _, DropdownMenu as _, PopupMenuItem, button,
    button_with_icon, icons, list_page, menu_builder, tab, tab_bar,
};
use crate::workspace::layout::{RightDockSnapshot, TaskProjects};

pub(in crate::workspace) fn header(
    snap: &RightDockSnapshot,
    close: AnyElement,
    cx: &App,
) -> AnyElement {
    list_page::header(
        strings::dock::right_tab_tasks(),
        crate::workspace::pages::chrome::project_picker(
            &crate::workspace::pages::scope::ScopeSnapshot {
                selected: snap.task_browser.scope.into(),
                current: snap.task_projects.active,
                projects: snap
                    .task_projects
                    .names
                    .iter()
                    .map(
                        |(uuid, name)| crate::workspace::pages::scope::ScopeProject {
                            uuid: *uuid,
                            name: name.clone(),
                        },
                    )
                    .collect(),
            },
            (*snap.workspace).clone(),
            cx,
        ),
        new_button(snap),
        close,
        cx,
    )
    .into_any_element()
}

fn project_label(scope: TaskScope, projects: &TaskProjects) -> String {
    if scope == TaskScope::AllProjects {
        return strings::common::page_all_projects();
    }
    scope
        .project(projects.active)
        .and_then(|id| projects.name(id))
        .map(str::to_owned)
        .unwrap_or_else(strings::task::project_not_open)
}

fn new_button(snap: &RightDockSnapshot) -> AnyElement {
    let workspace = snap.workspace.clone();
    let project = snap.task_browser.scope.project(snap.task_projects.active);
    let projects = snap.task_projects.names.clone();
    let button = button_with_icon("task-new", strings::task::new_button(), icons::ADD)
        .debug_selector(|| "task-new".into())
        .primary()
        .flex_none()
        .tab_stop(true)
        .disabled(
            projects.is_empty()
                || project.is_some_and(|id| !projects.iter().any(|(uuid, _)| *uuid == id)),
        );
    button
        .when(project.is_none(), |button| {
            button.tooltip(strings::common::choose_creation_project())
        })
        .on_click(move |_, window, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.new_task_in_scope(window, cx));
            }
        })
        .into_any_element()
}

pub(super) fn filter_label(filter: TaskFilter) -> String {
    match filter {
        TaskFilter::All => strings::task::filter_all(),
        TaskFilter::Backlog => strings::task::filter_backlog(),
        TaskFilter::Running => strings::task::filter_running(),
        TaskFilter::Done => strings::task::filter_done(),
        TaskFilter::Failed => strings::task::filter_failed(),
        TaskFilter::Cancelled => strings::task::filter_cancelled(),
    }
}

pub(super) fn status_tabs(snap: &RightDockSnapshot, list: &TaskList<'_>) -> impl IntoElement {
    let workspace = snap.workspace.clone();
    tab_bar("task-status-tabs")
        .w_full()
        .menu(true)
        .selected_index(
            FILTERS
                .iter()
                .position(|filter| *filter == snap.task_browser.state.filter)
                .unwrap_or(0),
        )
        .children(FILTERS.into_iter().zip(list.counts).enumerate().map(
            |(index, (filter, count))| {
                tab(strings::common::filter_count(filter_label(filter), count))
                    .debug_selector(move || format!("task-status-{index}"))
            },
        ))
        .on_click(move |index, _, cx| {
            if let Some(filter) = FILTERS.get(*index).copied()
                && let Some(ws) = workspace.upgrade()
            {
                ws.update(cx, |ws, cx| ws.set_task_filter(filter, cx));
            }
        })
}

pub(super) fn empty_state(snap: &RightDockSnapshot, scoped_count: usize, cx: &App) -> AnyElement {
    let project_available = snap.task_browser.scope == TaskScope::AllProjects
        || snap
            .task_browser
            .scope
            .project(snap.task_projects.active)
            .and_then(|id| snap.task_projects.name(id))
            .is_some();
    let message = if !project_available {
        strings::task::empty_project_unavailable()
    } else if scoped_count == 0 {
        strings::task::empty_scope(project_label(snap.task_browser.scope, &snap.task_projects))
    } else {
        strings::task::empty_filtered()
    };
    list_page::empty(message, cx).into_any_element()
}

pub(super) fn results(snap: &RightDockSnapshot, list: &TaskList<'_>, cx: &App) -> impl IntoElement {
    let filtered = snap.task_browser.state.filter != TaskFilter::All
        || !snap.task_browser.query.trim().is_empty();
    let workspace = snap.workspace.clone();
    let clear = filtered.then(|| {
        button("task-clear-filters", strings::task::clear_filters())
            .ghost()
            .debug_selector(|| "task-clear-filters".into())
            .tab_stop(true)
            .on_click(move |_, window, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update(cx, |ws, cx| ws.clear_task_filters(window, cx));
                }
            })
            .into_any_element()
    });
    list_page::results(
        strings::task::result_count(list.visible.len(), list.scoped_count),
        clear,
        cx,
    )
}

fn grouping_label(mode: TaskGrouping) -> String {
    match mode {
        TaskGrouping::None => strings::common::group_none(),
        TaskGrouping::Status => strings::common::column_status(),
        TaskGrouping::Project => strings::task::column_project(),
    }
}

pub(super) fn grouping_picker(snap: &RightDockSnapshot) -> impl IntoElement {
    let selected = snap.task_browser.state.groups.mode;
    let workspace = snap.workspace.clone();
    button(
        "task-grouping",
        strings::common::group_by(grouping_label(selected)),
    )
    .debug_selector(|| "task-grouping".into())
    .flex_none()
    .tab_stop(true)
    .child(icons::icon(icons::EXPAND_MORE))
    .dropdown_menu(menu_builder(move |menu, _, _| {
        [
            TaskGrouping::None,
            TaskGrouping::Status,
            TaskGrouping::Project,
        ]
        .into_iter()
        .fold(menu, |menu, mode| {
            let workspace = workspace.clone();
            menu.item(
                PopupMenuItem::new(grouping_label(mode))
                    .checked(mode == selected)
                    .on_click(move |_, _, cx| {
                        if let Some(ws) = workspace.upgrade() {
                            ws.update(cx, |ws, cx| ws.set_task_grouping(mode, cx));
                        }
                    }),
            )
        })
    }))
}

/// Named by the group rather than its position, so a group appearing or
/// vanishing leaves every other header's element state where it was.
fn group_id(part: &str, key: TaskGroupKey) -> SharedString {
    match key {
        TaskGroupKey::Status(filter) => format!("task-{part}-{filter:?}"),
        TaskGroupKey::Project(project) => format!("task-{part}-{}", project.as_inner()),
    }
    .into()
}

pub(super) fn group_header(
    group: &TaskGroup<'_>,
    snap: &RightDockSnapshot,
    cx: &App,
) -> impl IntoElement {
    let label = match group.key {
        TaskGroupKey::Status(filter) => filter_label(filter),
        TaskGroupKey::Project(project) => snap
            .task_projects
            .name(project)
            .map(str::to_owned)
            .unwrap_or_else(strings::task::project_not_open),
    };
    let key = group.key;
    let workspace = snap.workspace.clone();
    let id = group_id("group", key);
    list_page::group_header(
        id.clone(),
        group_id("group-chevron", key),
        strings::common::filter_count(label, group.tasks.len()),
        snap.task_browser.state.groups.is_open(key),
        cx,
    )
    .debug_selector(move || id.to_string())
    .on_click(move |_, _, cx| {
        if let Some(ws) = workspace.upgrade() {
            ws.update(cx, |ws, cx| ws.toggle_task_group(key, cx));
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_filters_keep_their_status_meaning() {
        assert_eq!(super::FILTERS[0], super::TaskFilter::All);
    }
}
