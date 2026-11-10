//! Handing a started task's prompt to its Agent chat pane, and answering
//! the start once the delivery settles.

use std::path::Path;

use chrono::Utc;
use daruda_chat::view::AgentSessionStatus;
use daruda_store::tasks::{ExecutionRef, Lifecycle, TaskEvent, TaskRecord};
use gpui::{Context, Window};

use super::{Answer, TaskStartError, TaskStarted, fail_start};
use crate::runs::{GlobalTasks, RunsHost};
use crate::workspace::Workspace;
use crate::workspace::main_area::pane_tree::PaneId;

impl Workspace {
    /// Answer after the preparation and delivery result is projected. Chat
    /// queueing leaves the task Dispatching until its PromptSent arrives.
    pub(super) fn answer_when_dispatched(
        &mut self,
        started: Result<(TaskStarted, gpui::Task<Result<(), TaskStartError>>), TaskStartError>,
        exec: ExecutionRef,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        let (started, dispatched) = match started {
            Ok(started) => started,
            Err(e) => {
                self.finish_task_start(Err(e), Some(&exec), &answer, cx);
                return;
            }
        };
        cx.spawn(async move |ws, cx| {
            let outcome = dispatched.await.map(|()| started);
            let finished = ws.update(cx, |ws, cx| {
                ws.finish_task_start(outcome.clone(), None, &answer, cx)
            });
            if finished.is_err() {
                // SILENT-OK: the caller may have stopped listening.
                let _ = answer.try_send(outcome);
            }
        })
        .detach();
    }

    /// Bind chat `pane_id` to the run, record the pane's identity, and hand
    /// it the prompt as an ACP turn. Preparation is durable before delivery;
    /// only the turn's tagged PromptSent records Dispatched.
    ///
    /// Takes `pane_id` rather than relying on `focused_pane_id`: a start
    /// asked for elsewhere opens its pane behind the one the window shows.
    pub(in crate::workspace::right_dock) fn dispatch_task_to_chat(
        &mut self,
        exec: &ExecutionRef,
        worktree_path: &Path,
        branch: &str,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<gpui::Task<Result<(), TaskStartError>>, TaskStartError> {
        // A cancel that landed while the lane was made ends the start here.
        let task = cx
            .global::<GlobalTasks>()
            .get(&exec.task_id)
            .filter(|task| matches!(&task.lifecycle, Lifecycle::Preparing { exec: current, .. } if current == exec))
            .cloned()
            .ok_or(TaskStartError::NotBacklog)?;
        let identity = self
            .bind_task_chat_execution(exec, pane_id, cx)
            .ok_or(TaskStartError::PaneUnavailable)?;
        let prepared = RunsHost::propose(
            TaskEvent::LanePrepared {
                terminal_id: None,
                exec: exec.clone(),
                worktree: worktree_path.to_path_buf(),
                agent_id: identity.agent_id,
                account_id: identity.account_id,
                cwd: identity.cwd,
                at: Utc::now(),
            },
            cx,
        );
        let exec = exec.clone();
        let branch = branch.to_owned();
        Ok(cx.spawn_in(window, async move |ws, cx| {
            prepared
                .await
                .map_err(|error| TaskStartError::NotRecorded {
                    detail: error.to_string(),
                })?;
            let result = ws
                .update_in(cx, |ws, window, cx| {
                    ws.deliver_prepared_task(&exec, &task, &branch, pane_id, window, cx)
                })
                .unwrap_or(Err(TaskStartError::WindowClosed));
            if let Err(error) = &result {
                gpui::AsyncApp::update(cx, |cx| fail_start(&exec, error, cx));
            }
            result
        }))
    }

    fn deliver_prepared_task(
        &mut self,
        exec: &ExecutionRef,
        task: &TaskRecord,
        branch: &str,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), TaskStartError> {
        if RunsHost::pane_execution(self.run_pane(pane_id), cx) != Some(exec) {
            return Err(TaskStartError::PaneUnavailable);
        }
        if !cx.global::<GlobalTasks>().get(&exec.task_id).is_some_and(|task| {
            matches!(&task.lifecycle, Lifecycle::Dispatching { exec: current, .. } if current == exec)
        }) {
            return Err(TaskStartError::NotBacklog);
        }
        let rendered = daruda_store::tasks::render_task_prompt(task, branch);
        let failed = self
            .agent_chat_view(pane_id)
            .is_some_and(|view| matches!(view.read(cx).status(), AgentSessionStatus::Error { .. }));
        let input = crate::workspace::main_area::pane_input_ops::PaneTextInput {
            body: rendered,
            intent: crate::workspace::main_area::pane_input_ops::PaneTextIntent::Command {
                submit: true,
            },
            task: Some(exec.clone()),
        };
        if failed || !self.deliver_text_to_pane(pane_id, input, window, cx) {
            RunsHost::delivery_failed(
                exec.clone(),
                crate::runs::DeliveryFailure::Dropped(
                    daruda_store::tasks::machine::UndeliveredReason::StartFailed,
                ),
                cx,
            );
            return Err(TaskStartError::PromptUndelivered);
        }
        cx.notify();
        Ok(())
    }
}
