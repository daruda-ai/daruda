//! What the task list is narrowed to — window-local, never persisted.

use daruda_store::tasks::TaskFilter;

use super::TaskGroups;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::workspace) struct TaskBrowserState {
    pub filter: TaskFilter,
    pub groups: TaskGroups,
}

#[cfg(test)]
mod tests {
    // Group and status transitions are covered by the list and page tests.
}
