//! Explicit project selection for aggregate task creation.

use crate::ui::{
    WindowExt as _,
    list::{FilteredItem, FilteredListState, ListEvent, searchable_list_state},
};
use crate::workspace::{ModalView, Workspace};
use daruda_store::project::ProjectUuid;
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window,
};

mod view;

#[derive(Clone)]
struct ProjectItem {
    project: ProjectUuid,
    label: SharedString,
}
impl FilteredItem for ProjectItem {
    fn label(&self) -> SharedString {
        self.label.clone()
    }
}

pub(super) struct NewTaskTarget {
    list: Entity<FilteredListState<ProjectItem>>,
    workspace: WeakEntity<Workspace>,
    _subscription: Subscription,
}

impl NewTaskTarget {
    fn new(
        workspace: WeakEntity<Workspace>,
        items: Vec<ProjectItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = cx.new(|cx| searchable_list_state(items, window, cx));
        let subscription = cx.subscribe_in(
            &list,
            window,
            |this, state, event: &ListEvent, window, cx| match event {
                ListEvent::Confirm(index) => {
                    let project = state
                        .read(cx)
                        .delegate()
                        .item_at(*index)
                        .map(|item| item.project);
                    this.confirm(project, window, cx);
                }
                ListEvent::Cancel => this.dismiss(window, cx),
                ListEvent::Select(_) => {}
            },
        );
        Self {
            list,
            workspace,
            _subscription: subscription,
        }
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut App) {
        window.close_dialog(cx);
    }

    fn confirm(
        &mut self,
        project: Option<ProjectUuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss(window, cx);
        if let Some(project) = project
            && let Some(ws) = self.workspace.upgrade()
        {
            ws.update(cx, |ws, cx| {
                ws.open_task_draft_for_project(project, window, cx)
            });
        }
    }
}

impl Focusable for NewTaskTarget {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.list.focus_handle(cx)
    }
}
impl ModalView for NewTaskTarget {}

impl Workspace {
    pub(in crate::workspace) fn choose_new_task_project(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items: Vec<_> = self
            .projects
            .iter()
            .map(|project| ProjectItem {
                project: project.uuid,
                label: project.name.clone().into(),
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let workspace = cx.weak_entity();
        crate::workspace::dialog_helpers::open_form_modal(
            crate::surface::strings::common::choose_creation_project(),
            None,
            move |window, cx| NewTaskTarget::new(workspace.clone(), items.clone(), window, cx),
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    // The palette and aggregate picker are exercised by pages::scope::tests.
}
