//! Starting and opening Tasks: the work a gated `TaskStart` / `TaskOpen`
//! runs once approved, and that `/task` reaches directly.

use gpui::App;

use crate::control::exec::{Caller, in_task_window};
use crate::control::result::{ControlError, ControlOutcome, ControlResult, LaneHandle};
use crate::remote_channel::bridge::PaneRef;
use crate::runs::GlobalTasks;
use crate::surface::strings as s;

use super::announce_chat;

/// A task's title, for the sentence that asks or tells about it: written by
/// whoever created the task, so flattened to a line that cannot reorder or
/// hide the rest of the sentence.
pub(super) fn task_title(task: &str, cx: &App) -> String {
    cx.global::<GlobalTasks>()
        .get(task)
        .map(|t| daruda_control_types::agent_text::title_line(&t.spec.title))
        .unwrap_or_default()
}

/// How an approval names the flow a Backlog `task` runs as, if it runs as
/// one: in the window that holds its project, where Start will find it.
pub(crate) fn task_flow_summary(task: &str, cx: &mut App) -> Option<String> {
    let record = cx.global::<GlobalTasks>().get(task)?.clone();
    let name = record.spec.flow.as_ref()?.name.clone();
    let found = in_task_window(task, cx, |ws, _, _| {
        Ok(ws.control_task_flow_approval(&record))
    });
    Some(
        found
            .ok()
            .flatten()
            .unwrap_or_else(|| s::control::approval_task_as_flow_unresolved(&name)),
    )
}

/// A start for the agent: approved already, so this only waits for it.
/// `agent` is the one its card named.
pub(super) async fn start_task(
    task: String,
    agent: String,
    caller: Caller,
    cx: &mut gpui::AsyncApp,
) -> ControlOutcome {
    cx.update(|cx| begin_task_start(task, caller, Some(agent), cx))?
        .finish(cx)
        .await
}

/// A task start under way, in the window that holds the task's project.
pub(crate) struct PendingTaskStart {
    pub(crate) task: String,
    pub(crate) title: String,
    caller: Caller,
    workspace: daruda_store::project::WorkspaceUuid,
    answer: smol::channel::Receiver<crate::workspace::ControlTaskStart>,
}

/// Start `task` and hand back what to wait on. A refusal taken before git
/// runs is already in the answer — see [`PendingTaskStart::try_finish`].
/// `agent` pins the chat's agent, as an approval card named it; `None`
/// resolves it as a new pane would.
pub(crate) fn begin_task_start(
    task: String,
    caller: Caller,
    agent: Option<String>,
    cx: &mut App,
) -> Result<PendingTaskStart, ControlError> {
    let title = task_title(&task, cx);
    let (workspace, answer) = in_task_window(&task, cx, |ws, window, cx| {
        let submitter = caller.flow_submitter();
        Ok((
            ws.uuid(),
            ws.control_task_start(&task, submitter, agent.clone(), window, cx),
        ))
    })?;
    Ok(PendingTaskStart {
        task,
        title,
        caller,
        workspace,
        answer,
    })
}

impl PendingTaskStart {
    /// The outcome if it is already known: a refusal, or a start that had
    /// no git to wait for.
    pub(crate) fn try_finish(&self, cx: &mut App) -> Option<ControlOutcome> {
        let outcome = self.answer.try_recv().ok()?;
        Some(self.started(outcome, cx))
    }

    pub(crate) async fn finish(self, cx: &mut gpui::AsyncApp) -> ControlOutcome {
        let outcome = self
            .answer
            .recv()
            .await
            .unwrap_or(Err(ControlError::TargetGone));
        cx.update(|cx| self.started(outcome, cx))
    }

    /// Announce a started task's chat with the start request's provenance.
    fn started(&self, outcome: crate::workspace::ControlTaskStart, cx: &mut App) -> ControlOutcome {
        let (exec, lane, pane) = outcome?;
        self.caller
            .scope_run(crate::runs::result::RunKey::Task(exec.clone()), cx);
        let chat = pane.map(|pane| PaneRef {
            workspace: self.workspace,
            pane,
        });
        if let Some(chat) = chat {
            announce_chat(chat, self.caller, cx);
        }
        Ok(ControlResult::TaskStarted {
            handle: crate::control::mcp::runs::run_handle(crate::runs::result::RunKey::Task(exec)),
            task: self.task.clone(),
            lane: LaneHandle::new(self.workspace, lane),
            chat,
        })
    }
}

/// Bring up the chat `task` ran in and announce it to remote channels. A
/// new pane takes the mode the caller's chats take.
pub(crate) fn open_task(task: String, caller: Caller, cx: &mut App) -> ControlOutcome {
    let mode = crate::workspace::mode_for(caller.origin().started_by());
    let chat = in_task_window(&task, cx, |ws, window, cx| {
        let pane = ws.control_task_open(&task, mode.clone(), window, cx)?;
        Ok(PaneRef {
            workspace: ws.uuid(),
            pane,
        })
    })?;
    announce_chat(chat, caller, cx);
    Ok(ControlResult::TaskOpened { task, chat })
}
