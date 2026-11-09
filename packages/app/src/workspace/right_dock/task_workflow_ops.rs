//! Right-panel Tasks tab — lifecycle layer.
//!
//! Hosts the task workflows after Start (which is `task_start`'s):
//! `cancel_task`, `focus_task_lane`, `reopen_task`, `retry_task`, plus
//! the session bookkeeping of Claude CLI task runs
//! (`apply_task_session_changed`, `apply_task_session_ended`).
//!
//! Sibling of `task_ops` (filter + CRUD). Both modules
//! extend the same `Workspace` via separate `impl Workspace` blocks.

use std::path::Path;
use std::time::Duration;

use crate::runs::{GlobalTasks, RunsHost};
use chrono::Utc;
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::tasks::machine::SettleOutcome;
use daruda_store::tasks::{Lifecycle, TaskEvent};
use gpui::{Context, Window};

use crate::workspace::Workspace;

pub(in crate::workspace) const TASK_LIVE_TICK: Duration = Duration::from_millis(250);

impl Workspace {
    pub(in crate::workspace) fn after_task_projection(
        &mut self,
        operation: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.claude_sessions.task_hook_tail.is_closed() && RunsHost::settled(cx) {
            operation(self, cx);
            return;
        }
        let (done, next) = smol::channel::bounded(1);
        let previous = std::mem::replace(&mut self.claude_sessions.task_hook_tail, next);
        cx.spawn(async move |workspace, cx| {
            // SILENT-OK: closure means the previous hook finished.
            let _ = previous.recv().await;
            let mut operation = Some(operation);
            loop {
                let pending = cx.update(|cx| {
                    if RunsHost::settled(cx) {
                        if let Some(operation) = operation.take()
                            && let Err(error) = workspace.update(cx, operation)
                        {
                            LogWriter::log(
                                ErrorReport::new("Task hook could not reach its workspace")
                                    .severity(ErrorSeverity::Warning)
                                    .message(error.to_string())
                                    .at(file!(), line!())
                                    .dedup("tasks.hook.workspace")
                                    .build(),
                            );
                        }
                        None
                    } else {
                        Some(RunsHost::barrier(cx))
                    }
                });
                let Some(pending) = pending else { break };
                pending.await;
            }
            drop(done);
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Lifecycle (start / cancel / focus / reopen / retry)
    // ------------------------------------------------------------------

    /// Per-repo lock acquisition. Two concurrent `start_task` calls
    /// against the same repo race on `git worktree add`; we make the
    /// second one fail fast with a user-visible error rather than
    /// risk a half-created lane.
    ///
    /// `pub(super)` because the race is a property of the
    /// repository, not of this path: an agent-requested creation
    /// (`control_lane_ops`) has to take the same lock or it can interleave
    /// with the user's own.
    pub(in crate::workspace) fn acquire_repo_lock(&mut self, repo_root: &Path) -> bool {
        self.pending_lane_creates.insert(repo_root.to_path_buf())
    }

    pub(in crate::workspace) fn release_repo_lock(&mut self, repo_root: &Path) {
        self.pending_lane_creates.remove(repo_root);
    }

    /// Resolve the branch name we should base the new lane on.
    /// Looks the path up in the workspace's lane list and returns
    /// `lane.branch.clone()` when it's a git worktree. `None`
    /// is passed to `resolve_lane_base_ref`, which falls back to
    /// the project's `base_branch`, then `default_branch`, then
    /// the current HEAD.
    pub(super) fn branch_for_worktree_path(&self, path: &Path) -> Option<String> {
        self.projects
            .iter()
            .flat_map(|p| &p.lanes)
            .find(|w| w.path == path)
            .and_then(|w| match &w.kind {
                daruda_store::project::LaneKind::Git { branch, .. } => branch.clone(),
                _ => None,
            })
    }

    /// User-initiated stop. Lane is preserved (D-1). The log records the
    /// cancel before the run's pane is told to stop.
    pub(in crate::workspace) fn cancel_task(&mut self, task_id: &str, cx: &mut Context<Self>) {
        let Some(task) = cx.global::<GlobalTasks>().get(task_id) else {
            return;
        };
        match task.current_exec().cloned() {
            Some(exec) => self.cancel_execution(exec, cx).detach(),
            // Nothing runs, so the log refuses; the refusal is the report.
            None => {
                let refused = crate::runs::TaskCommitError::Rejected(
                    daruda_store::tasks::TaskRejected::IllegalTransition {
                        from: task_kind(cx, task_id),
                        event: daruda_store::tasks::machine::EventKind::CancelRequested,
                    },
                );
                self.report_error(super::task_commit_ops::task_commit_report(&refused), cx);
            }
        }
    }

    /// Switch the active lane to the one this task is running in. A running
    /// task whose checkout was deleted from the outside is interrupted
    /// (`LaneLost`) rather than dangling in `Running` forever; a finished
    /// task keeps its state.
    pub(in crate::workspace) fn focus_task_lane(
        &mut self,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = cx.global::<GlobalTasks>().get(task_id) else {
            return;
        };
        let Some(path) = task.lifecycle.worktree().map(ToOwned::to_owned) else {
            return;
        };
        if !path.exists() {
            if let Lifecycle::Running { exec, .. } = &task.lifecycle {
                let event = TaskEvent::LaneLost {
                    exec: exec.clone(),
                    at: Utc::now(),
                };
                RunsHost::propose_fact(event, cx);
            }
            return;
        }

        // In any project: a task's lane need not be in the active one.
        if let Some(target) = self.lane_ref_at(&path) {
            self.activate_lane(target, window, cx);
        }
    }

    /// Move a terminal-state task back to `Backlog`; the next [Start] runs
    /// it again (see `return_task_to_backlog` for where).
    pub(in crate::workspace) fn reopen_task(&mut self, task_id: &str, cx: &mut Context<Self>) {
        self.return_task_to_backlog(task_id, cx).detach();
    }

    /// `Reopen` + `start_task` in one click — for the `[Retry]`
    /// affordance on failed rows. The start waits for the reopen to land.
    pub(super) fn retry_task(
        &mut self,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reopened = self.return_task_to_backlog(task_id, cx);
        let id = task_id.to_owned();
        cx.spawn_in(window, async move |ws, cx| {
            if reopened.await.is_err() {
                return;
            }
            // SILENT-OK: the window closed; nothing is left to start in.
            let _ = ws.update_in(cx, |ws, window, cx| ws.start_task(&id, window, cx));
        })
        .detach();
    }

    /// Put a finished task back in Backlog so it can run again — in the lane
    /// its earlier run created, while that lane is still registered.
    fn return_task_to_backlog(
        &mut self,
        task_id: &str,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<u64, crate::runs::TaskCommitError>> {
        let task = cx.global::<GlobalTasks>().get(task_id);
        let run_in = task.map(|task| match self.own_lane_for_rerun(task) {
            Some(path) => daruda_store::tasks::TaskRunIn::ExistingLane { path },
            None => task.spec.run_in.clone(),
        });
        let intent = crate::runs::TaskIntent::Reopen {
            id: task_id.to_owned(),
            run_in: run_in.unwrap_or_default(),
        };
        self.propose_task_command(intent, cx)
    }

    // ------------------------------------------------------------------
    // Claude hook → task state mapping
    // ------------------------------------------------------------------

    /// Attach `session_id` to the running CLI task whose run owns it.
    /// Proposed only when it is not attached yet, so the same hook firing
    /// twice does not grow the session list.
    pub(in crate::workspace) fn apply_task_session_changed(
        &mut self,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let session_id = session_id.to_owned();
        self.after_task_projection(
            move |_, cx| RunsHost::attach_cli_session(&session_id, cx),
            cx,
        );
    }

    /// Settle every running CLI task that owns `session_id` as `Done` /
    /// `Error`, depending on `reason`. Settling clears its session list, so a
    /// future `Retry` starts clean.
    pub(in crate::workspace) fn apply_task_session_ended(
        &mut self,
        session_id: &str,
        reason: daruda_store::tasks::SessionEndReason,
        cx: &mut Context<Self>,
    ) {
        let session_id = session_id.to_owned();
        self.after_task_projection(
            move |_, cx| RunsHost::settle_cli_session(&session_id, reason, cx),
            cx,
        );
    }

    /// Settle the task whose run `pane_id` owns, from that pane's activity
    /// edge. Only the pane that dispatched this execution may settle it;
    /// restoring history or a stale execution's late event owns no new work.
    pub(in crate::workspace) fn settle_agent_chat_task(
        &mut self,
        pane_id: crate::workspace::main_area::pane_tree::PaneId,
        outcome: SettleOutcome,
        result: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(exec) = self.task_chat_execution(pane_id, cx) else {
            // A continued run binds its pane before its write lands, so a turn
            // that ends first finds the pane ahead of the log: settle once the
            // log has caught up.
            if !RunsHost::settled(cx)
                && RunsHost::pane_execution(self.run_pane(pane_id), cx).is_some()
            {
                let caught_up = RunsHost::barrier(cx);
                cx.spawn(async move |ws, cx| {
                    caught_up.await;
                    // SILENT-OK: a closed window's panes have no run to settle.
                    let _ = ws.update(cx, |ws, cx| {
                        if let Some(exec) = ws.task_chat_execution(pane_id, cx) {
                            RunsHost::settle_execution(exec, outcome, result, cx);
                        }
                    });
                })
                .detach();
            }
            return;
        };
        RunsHost::settle_execution(exec, outcome, result, cx);
    }

    pub(in crate::workspace) fn fail_agent_chat_start(
        &mut self,
        pane_id: crate::workspace::main_area::pane_tree::PaneId,
        cause: daruda_store::tasks::machine::StartFailure,
        cx: &mut Context<Self>,
    ) {
        self.release_cli_resume(pane_id);
        if let Some(exec) = RunsHost::pane_execution(self.run_pane(pane_id), cx).cloned() {
            RunsHost::delivery_failed(exec, crate::runs::DeliveryFailure::Start(cause), cx);
            if let Some(view) = self.agent_chat_view(pane_id).cloned() {
                view.update(cx, |view, cx| {
                    view.discard_pending_prompts(
                        daruda_chat::view::PromptDropReason::ConnectFailed,
                        cx,
                    )
                });
            }
        }
    }

    pub(in crate::workspace) fn fail_agent_chat_task(
        &mut self,
        pane_id: crate::workspace::main_area::pane_tree::PaneId,
        cx: &mut Context<Self>,
    ) {
        if let Some(exec) = RunsHost::pane_execution(self.run_pane(pane_id), cx).cloned() {
            RunsHost::delivery_failed(
                exec,
                crate::runs::DeliveryFailure::Dropped(
                    daruda_store::tasks::machine::UndeliveredReason::StartFailed,
                ),
                cx,
            );
        }
    }
}

fn task_kind(cx: &gpui::App, task_id: &str) -> daruda_store::tasks::LifecycleKind {
    cx.global::<GlobalTasks>()
        .get(task_id)
        .map_or(daruda_store::tasks::LifecycleKind::Backlog, |task| {
            task.lifecycle.kind()
        })
}

/// Background loop that drives the Tasks-tab pulse + duration
/// updates. Self-terminates as soon as the global task list contains
/// no `Running` row, so the workspace doesn't burn wakeups while
/// every task is idle.
pub(super) fn spawn_task_live_tick(cx: &mut Context<Workspace>) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(TASK_LIVE_TICK).await;
            let still_alive = this
                .update(cx, |ws, cx| {
                    let running_exists = cx.global::<GlobalTasks>().any_active();
                    if running_exists {
                        cx.notify();
                        ws.notify_right_dock(cx);
                    }
                    running_exists
                })
                .unwrap_or(false);
            if !still_alive {
                break;
            }
        }
    })
}
