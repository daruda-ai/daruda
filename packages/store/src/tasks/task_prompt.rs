//! The text a task run sends as its first agent turn.

use super::machine::{LifecycleKind, TaskRecord};

/// Wrap the user's raw prompt with task metadata + a closing instruction
/// asking for a final summary. Lane agents have no tool that updates a task;
/// their last answer is what the run records as the task's result. `branch`
/// is what the header names — the caller resolves it, since an existing
/// lane's branch is read at Start.
pub fn render_task_prompt(task: &TaskRecord, branch: &str) -> String {
    let state_label = match task.lifecycle.kind() {
        LifecycleKind::Backlog => "Backlog",
        LifecycleKind::Preparing | LifecycleKind::Dispatching => "Starting",
        LifecycleKind::Running => "Running",
        LifecycleKind::Done => "Done",
        LifecycleKind::Error => "Error",
        LifecycleKind::Cancelled => "Cancelled",
        LifecycleKind::Undelivered => "Undelivered",
        LifecycleKind::Interrupted => "Interrupted",
    };
    let spec = &task.spec;
    let description = task.description.as_ref().filter(|_| {
        matches!(
            task.lifecycle.kind(),
            LifecycleKind::Preparing | LifecycleKind::Dispatching | LifecycleKind::Running
        )
    });
    let (title, prompt) = description.map_or((&spec.title, &spec.prompt), |description| {
        (&description.title, &description.prompt)
    });
    format!(
        "Task: \"{title}\" ({branch})\n\
         Status: {status}\n\
         \n\
         {body}\n\
         \n\
         {CLOSING_INSTRUCTION}\n",
        title = title,
        status = state_label,
        body = prompt,
    )
}

const CLOSING_INSTRUCTION: &str = "Work in the current workspace. Inspect the relevant \
    code, make the needed changes, and verify them when practical. When done, end with \
    a short summary of what you changed and how you verified it; that final answer is \
    recorded as the task's result.";
