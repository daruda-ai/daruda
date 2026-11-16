//! The task list's browsing state with its search input, and the copy a
//! render pass reads.

use gpui::{AppContext as _, Context, Entity, Window};

use super::state::TaskBrowserState;
use crate::ui::InputState;
use crate::workspace::{Workspace, layout::diff_policy::Handle};

pub(in crate::workspace) struct TaskBrowser {
    pub state: TaskBrowserState,
    /// Substring-filters rows over title, prompt, notes, branch and subtasks.
    pub search: Entity<InputState>,
    /// Registered targets, refreshed by task and project/lane transitions.
    pub worktrees:
        std::collections::HashMap<daruda_store::tasks::TaskId, daruda_store::project::LaneRef>,
}

impl TaskBrowser {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            state: TaskBrowserState::default(),
            worktrees: Default::default(),
            search: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::surface::strings::task::search_placeholder())
            }),
        }
    }

    pub fn snapshot(
        &self,
        scope: daruda_store::tasks::TaskScope,
        cx: &gpui::App,
    ) -> TaskBrowserSnapshot {
        TaskBrowserSnapshot {
            scope,
            worktrees: self.worktrees.clone(),
            state: self.state.clone(),
            search: Handle(self.search.clone()),
            query: self.search.read(cx).value().to_string(),
        }
    }
}

#[derive(PartialEq)]
pub(in crate::workspace) struct TaskBrowserSnapshot {
    pub scope: daruda_store::tasks::TaskScope,
    /// Registered targets, refreshed by task and project/lane transitions.
    pub worktrees:
        std::collections::HashMap<daruda_store::tasks::TaskId, daruda_store::project::LaneRef>,
    pub state: TaskBrowserState,
    pub search: Handle<Entity<InputState>>,
    /// The search text at snapshot time, so render never reads the input.
    pub query: String,
}
