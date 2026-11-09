use std::collections::{HashMap, HashSet};

use daruda_store::project::WorkspaceUuid;
use daruda_store::tasks::machine::{SettleOutcome, StartFailure, UndeliveredReason};
use daruda_store::tasks::{ExecutionRef, Lifecycle, TaskEvent};
use gpui::{App, BorrowAppContext as _};

use super::{GlobalTasks, RunsHost};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RunPane {
    pub(crate) workspace: WorkspaceUuid,
    pub(crate) pane: u64,
}

/// Which pane carries which task execution, and the executions settling.
#[derive(Default)]
pub(super) struct ExecutionBindings {
    pub(super) bindings: HashMap<RunPane, ExecutionRef>,
    pub(super) settling: HashSet<ExecutionRef>,
}

#[derive(Clone, Copy)]
pub(crate) enum DeliveryFailure {
    Start(StartFailure),
    Dropped(UndeliveredReason),
}

impl RunsHost {
    pub(crate) fn bind_execution(pane: RunPane, exec: ExecutionRef, cx: &mut App) {
        cx.update_global::<Self, _>(|host, _| {
            host.executions.bindings.insert(pane, exec);
        });
    }

    pub(crate) fn pane_execution(pane: RunPane, cx: &App) -> Option<&ExecutionRef> {
        cx.global::<Self>().executions.bindings.get(&pane)
    }

    pub(crate) fn unbind_execution(pane: RunPane, cx: &mut App) -> Option<ExecutionRef> {
        cx.update_global::<Self, _>(|host, _| host.executions.bindings.remove(&pane))
    }

    pub(crate) fn release_execution(pane: RunPane, reason: UndeliveredReason, cx: &mut App) {
        if let Some(exec) = Self::unbind_execution(pane, cx) {
            Self::delivery_failed(exec, DeliveryFailure::Dropped(reason), cx);
        }
    }

    pub(crate) fn release_workspace(workspace: WorkspaceUuid, cx: &mut App) {
        let panes: Vec<_> = cx
            .global::<Self>()
            .executions
            .bindings
            .keys()
            .filter(|pane| pane.workspace == workspace)
            .copied()
            .collect();
        for pane in panes {
            Self::release_execution(pane, UndeliveredReason::PaneClosed, cx);
        }
        Self::release_terminals(workspace, cx);
    }

    pub(crate) fn prompt_sent(exec: ExecutionRef, session_id: Option<String>, cx: &mut App) {
        Self::propose_fact(
            TaskEvent::Dispatched {
                exec,
                session_id,
                at: chrono::Utc::now(),
            },
            cx,
        );
    }

    pub(crate) fn prompt_dropped(exec: ExecutionRef, reason: UndeliveredReason, cx: &mut App) {
        Self::propose_fact(
            TaskEvent::Undelivered {
                exec,
                reason,
                at: chrono::Utc::now(),
            },
            cx,
        );
    }

    pub(crate) fn delivery_failed(exec: ExecutionRef, failure: DeliveryFailure, cx: &mut App) {
        let running = cx
            .global::<GlobalTasks>()
            .get(&exec.task_id)
            .is_some_and(|task| matches!(task.lifecycle, Lifecycle::Running { .. }));
        if running && Self::settled(cx) {
            apply_failure(exec, failure, cx);
        } else {
            cx.defer(move |cx| {
                Self::after_projection(move |cx| apply_failure(exec, failure, cx), cx)
            });
        }
    }

    pub(crate) fn settle_execution(
        exec: ExecutionRef,
        outcome: SettleOutcome,
        result: Option<String>,
        cx: &mut App,
    ) {
        let running = cx
            .global::<GlobalTasks>()
            .get(&exec.task_id)
            .is_some_and(|task| matches!(task.lifecycle, Lifecycle::Running { .. }));
        let operation = move |cx: &mut App| {
            if cx.global::<GlobalTasks>().get(&exec.task_id).is_some_and(|task| matches!(&task.lifecycle, Lifecycle::Running { exec: current, .. } if current == &exec)) {
                Self::propose_fact(TaskEvent::Settled { result, exec, outcome, at: chrono::Utc::now() }, cx);
            }
        };
        if running {
            Self::after_projection(operation, cx);
        } else {
            cx.defer(move |cx| Self::after_projection(operation, cx));
        }
    }

    pub(super) fn after_projection(operation: impl FnOnce(&mut App) + 'static, cx: &mut App) {
        if Self::settled(cx) {
            operation(cx);
            return;
        }
        cx.spawn(async move |cx| {
            let mut operation = Some(operation);
            loop {
                let barrier = cx.update(|cx| {
                    if Self::settled(cx) {
                        if let Some(operation) = operation.take() {
                            operation(cx);
                        }
                        None
                    } else {
                        Some(Self::barrier(cx))
                    }
                });
                let Some(barrier) = barrier else { break };
                barrier.await;
            }
        })
        .detach();
    }
}

fn apply_failure(exec: ExecutionRef, failure: DeliveryFailure, cx: &mut App) {
    let Some(task) = cx
        .global::<GlobalTasks>()
        .get(&exec.task_id)
        .filter(|task| task.current_exec() == Some(&exec))
    else {
        return;
    };
    let at = chrono::Utc::now();
    let event = match (&task.lifecycle, failure) {
        (
            Lifecycle::Preparing { .. } | Lifecycle::Dispatching { .. },
            DeliveryFailure::Start(cause),
        ) => TaskEvent::DispatchFailed { exec, cause, at },
        (Lifecycle::Preparing { .. }, DeliveryFailure::Dropped(_)) => TaskEvent::DispatchFailed {
            exec,
            cause: StartFailure::PaneUnavailable,
            at,
        },
        (Lifecycle::Dispatching { .. }, DeliveryFailure::Dropped(reason)) => {
            TaskEvent::Undelivered { exec, reason, at }
        }
        (Lifecycle::Running { .. }, _) => TaskEvent::Settled {
            result: None,
            exec,
            outcome: SettleOutcome::Errored,
            at,
        },
        _ => return,
    };
    RunsHost::propose_fact(event, cx);
}
