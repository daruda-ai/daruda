//! Refresh link availability at task and project/lane transitions, never during render.

use crate::{runs::GlobalTasks, workspace::Workspace};
use gpui::App;

impl Workspace {
    pub(in crate::workspace) fn refresh_task_worktrees(&mut self, cx: &App) {
        let mut targets = std::collections::HashMap::new();
        self.right_views.tasks.worktrees = cx
            .global::<GlobalTasks>()
            .book()
            .records()
            .filter_map(|task| {
                let path = task.lifecycle.worktree()?;
                let lane = *targets
                    .entry(path.to_path_buf())
                    .or_insert_with(|| self.lane_ref_at(path).filter(|_| path.is_dir()));
                lane.map(|lane| (task.id().to_owned(), lane))
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    // Rendered link lifecycle regressions live in workspace::tests::pages.
}
