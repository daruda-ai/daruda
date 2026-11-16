//! Navigation shared by every workspace page, including its detail.

use gpui::{AnyElement, App, IntoElement, WeakEntity, div, prelude::*, px};

use super::scope::{ProjectScope, ScopeSnapshot};
use crate::surface::strings as s;
use crate::ui::{
    ButtonVariants as _, DropdownMenu as _, PopupMenuItem, button, button_bare, button_compact,
    icons, menu_builder, theme,
};
use crate::workspace::Workspace;

pub(in crate::workspace) fn project_picker(
    scope: &ScopeSnapshot,
    workspace: WeakEntity<Workspace>,
    cx: &App,
) -> AnyElement {
    let scope = scope.clone();
    let selected = scope.selected.project(scope.current);
    let label = if scope.selected == ProjectScope::All {
        s::common::page_all_projects()
    } else {
        scope
            .projects
            .iter()
            .find(|p| Some(p.uuid) == selected)
            .map(|p| p.name.clone())
            .unwrap_or_else(s::task::project_not_open)
    };
    button_bare("page-project-scope")
        .debug_selector(|| "page-project-scope".into())
        .tab_stop(true)
        .min_w_0()
        .tooltip(s::common::page_project_hint())
        .child(icons::icon(icons::FOLDER).text_color(theme::current(cx).text_muted))
        .child(
            div()
                .min_w_0()
                .max_w(px(theme::list_metrics::PROJECT_W))
                .truncate()
                .child(label),
        )
        .child(icons::icon(icons::EXPAND_MORE))
        .dropdown_menu(menu_builder(move |menu, _, _| {
            let all_ws = workspace.clone();
            let menu = menu.scrollable(true).item(
                PopupMenuItem::new(s::common::page_all_projects())
                    .checked(scope.selected == ProjectScope::All)
                    .on_click(move |_, window, cx| {
                        if let Some(ws) = all_ws.upgrade() {
                            ws.update(cx, |ws, cx| {
                                ws.select_page_scope(ProjectScope::All, window, cx)
                            });
                        }
                    }),
            );
            scope.projects.iter().fold(menu, |menu, project| {
                let workspace = workspace.clone();
                let uuid = project.uuid;
                menu.item(
                    PopupMenuItem::new(project.name.clone())
                        .checked(scope.selected != ProjectScope::All && selected == Some(uuid))
                        .on_click(move |_, window, cx| {
                            if let Some(ws) = workspace.upgrade() {
                                ws.update(cx, |ws, cx| {
                                    ws.select_page_scope(ProjectScope::Project(uuid), window, cx)
                                });
                            }
                        }),
                )
            })
        }))
        .into_any_element()
}

pub(in crate::workspace) fn return_button(workspace: WeakEntity<Workspace>) -> AnyElement {
    button("close-workspace-page", s::common::return_to_worktree())
        .debug_selector(|| "close-workspace-page".into())
        .ghost()
        .flex_none()
        .tab_stop(true)
        .on_click(move |_, window, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| ws.return_to_worktree(window, cx));
            }
        })
        .into_any_element()
}

pub(in crate::workspace) fn worktree_button(
    id: impl Into<gpui::ElementId>,
    lane: daruda_store::project::LaneRef,
    label: String,
    workspace: WeakEntity<Workspace>,
) -> AnyElement {
    div()
        .flex()
        .min_w_0()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            button_compact(id, label.clone())
                .ghost()
                .max_w_full()
                .justify_start()
                .tooltip(label)
                .tab_stop(true)
                .on_click(move |_, window, cx| {
                    if let Some(ws) = workspace.upgrade() {
                        ws.update(cx, |ws, cx| ws.activate_lane(lane, window, cx));
                    }
                }),
        )
        .into_any_element()
}

pub(in crate::workspace) fn resume_button(
    ws: &Workspace,
    cx: &gpui::Context<Workspace>,
) -> Option<AnyElement> {
    let page = ws.last_page()?;
    let workspace = cx.weak_entity();
    Some(
        button(
            "resume-workspace-page",
            s::common::resume_page(page.label()),
        )
        .debug_selector(|| "resume-workspace-page".into())
        .ml_auto()
        .ghost()
        .flex_none()
        .tab_stop(true)
        .child(icons::icon(page.icon()))
        .on_click(move |_, window, cx| {
            if let Some(ws) = workspace.upgrade() {
                ws.update(cx, |ws, cx| {
                    ws.on_resume_workspace_page(&crate::workspace::ResumeWorkspacePage, window, cx)
                });
            }
        })
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_scope_and_worktree_return_have_distinct_labels() {
        assert_ne!(
            super::s::common::page_all_projects(),
            super::s::common::return_to_worktree()
        );
    }
}
