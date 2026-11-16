//! Shared page browsing context. It never changes the worktree running the
//! user's terminals, chats or flows.

use daruda_store::project::{ProjectId, ProjectUuid};
use daruda_store::tasks::TaskScope;
use gpui::{Context, Window};

use super::Page;
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) enum ProjectScope {
    #[default]
    Current,
    All,
    Project(ProjectUuid),
}

impl ProjectScope {
    pub fn project(self, current: Option<ProjectUuid>) -> Option<ProjectUuid> {
        match self {
            Self::Current => current,
            Self::All => None,
            Self::Project(project) => Some(project),
        }
    }

    pub fn matches(self, project: ProjectUuid, current: Option<ProjectUuid>) -> bool {
        self == Self::All || self.project(current) == Some(project)
    }
}

impl From<TaskScope> for ProjectScope {
    fn from(scope: TaskScope) -> Self {
        match scope {
            TaskScope::ActiveProject => Self::Current,
            TaskScope::AllProjects => Self::All,
            TaskScope::Project(project) => Self::Project(project),
        }
    }
}

impl From<ProjectScope> for TaskScope {
    fn from(scope: ProjectScope) -> Self {
        match scope {
            ProjectScope::Current => Self::ActiveProject,
            ProjectScope::All => Self::AllProjects,
            ProjectScope::Project(project) => Self::Project(project),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::workspace) struct ScopeProject {
    pub uuid: ProjectUuid,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::workspace) struct ScopeSnapshot {
    pub selected: ProjectScope,
    pub current: Option<ProjectUuid>,
    pub projects: Vec<ScopeProject>,
}

impl Workspace {
    pub(in crate::workspace) fn page_scope(&self) -> ProjectScope {
        self.pages.scope
    }

    pub(in crate::workspace) fn scope_snapshot(&self) -> ScopeSnapshot {
        ScopeSnapshot {
            selected: self.page_scope(),
            current: self.active_project().map(|project| project.uuid),
            projects: self
                .projects
                .iter()
                .map(|project| ScopeProject {
                    uuid: project.uuid,
                    name: project.name.clone(),
                })
                .collect(),
        }
    }

    pub(in crate::workspace) fn page_scope_matches(&self, project: ProjectId) -> bool {
        self.project_for(project).is_some_and(|project| {
            self.page_scope().matches(
                project.uuid,
                self.active_project().map(|current| current.uuid),
            )
        })
    }

    pub(in crate::workspace) fn page_project(&self) -> Option<ProjectId> {
        let current = self.active_project().map(|project| project.uuid);
        self.page_scope()
            .project(current)
            .and_then(|uuid| self.project_by_uuid(uuid))
            .map(|project| project.id)
    }

    pub(in crate::workspace) fn reveal_page_project(
        &mut self,
        project: ProjectId,
        cx: &mut Context<Self>,
    ) {
        if !self.page_scope_matches(project)
            && let Some(project) = self.project_for(project)
        {
            self.set_page_scope(ProjectScope::Project(project.uuid), cx);
        }
    }

    /// The same project gesture in expanded and compact docks: browse it
    /// while a page is shown, enter its last worktree otherwise.
    pub(in crate::workspace) fn select_project(
        &mut self,
        project: ProjectId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project_for(project) else {
            return;
        };
        if self.active_page().is_some() {
            self.select_page_scope(ProjectScope::Project(project.uuid), window, cx);
        } else {
            let target = daruda_store::project::LaneRef {
                project: project.id,
                lane: project.last_active_lane_id,
            };
            self.activate_lane(target, window, cx);
        }
    }

    pub(in crate::workspace) fn select_page_scope(
        &mut self,
        scope: ProjectScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if scope == self.page_scope() {
            return;
        }
        if let ProjectScope::Project(project) = scope
            && self.project_by_uuid(project).is_none()
        {
            return;
        }
        if let Some(page) = self.active_page()
            && self.shown_page_detail_id().is_some()
        {
            self.leave_page_detail_then(page, window, cx, move |ws, _, cx| {
                ws.set_page_scope(scope, cx);
            });
        } else {
            self.set_page_scope(scope, cx);
        }
    }

    /// The one write of page scope; effects follow it even when a task
    /// reveal or a screenshot selects a project without a picker gesture.
    pub(in crate::workspace) fn set_page_scope(
        &mut self,
        scope: ProjectScope,
        cx: &mut Context<Self>,
    ) {
        if self.pages.scope == scope {
            return;
        }
        let current = self.active_project().map(|project| project.uuid);
        let project_changed = self.pages.scope.project(current) != scope.project(current)
            || (self.pages.scope == ProjectScope::All) != (scope == ProjectScope::All);
        self.pages.scope = scope;
        self.pages.flows.history_request = None;
        self.cancel_flow_start_navigation();
        if project_changed {
            self.flows.browser.state.scope = crate::workspace::flow_browser::FlowScope::All;
        }
        if self.active_page() == Some(Page::Flows) {
            self.respawn_flow_watcher(cx);
        }
        if self.active_page() == Some(Page::PullRequests) {
            crate::github::PrList::wanted(cx);
        }
        cx.notify();
    }

    pub(super) fn pin_page_scope(&mut self, cx: &mut Context<Self>) {
        if self.pages.scope == ProjectScope::Current
            && let Some(project) = self.active_project()
        {
            self.set_page_scope(ProjectScope::Project(project.uuid), cx);
        }
    }
}

#[cfg(test)]
mod tests;
