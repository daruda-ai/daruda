//! MCP-only execution queries and conversion from process-owned results.
use crate::runs::{RunsHost, result as domain};
use daruda_control_types::runs as wire;
use gpui::App;

enum StatusItem {
    /// A staged read, and whether its result was kept across a restart.
    Query(crate::runs::result_host::RunQuery, bool),
    Ready(wire::RunResult),
}
pub(crate) struct StatusQuery {
    runs: Vec<StatusItem>,
    errors: Vec<serde_json::Value>,
}

pub(super) fn resolve(
    handles: Option<Vec<String>>,
    cx: &mut App,
) -> (
    Vec<domain::RunKey>,
    Vec<wire::RunResult>,
    Vec<serde_json::Value>,
) {
    let mut keys = Vec::new();
    let mut ready = Vec::new();
    let mut errors = Vec::new();
    if let Some(handles) = handles {
        for raw in handles {
            let key = match raw.parse::<wire::RunHandle>() {
                Ok(wire::RunHandle::TaskLatest { task_id }) => {
                    RunsHost::latest_task_key(&task_id, cx).or_else(|| {
                        ready.push(wire::RunResult {
                            handle: wire::RunHandle::TaskLatest { task_id },
                            what: String::new(),
                            why: None,
                            state: wire::RunState::Gone,
                            recovered: false,
                        });
                        None
                    })
                }
                Ok(handle) => run_key(handle),
                Err(_) => {
                    errors.push(serde_json::json!({"code":"unknown_handle", "handle":raw}));
                    None
                }
            };
            if let Some(key) = key
                && !keys.contains(&key)
            {
                keys.push(key);
            }
        }
    } else {
        keys = RunsHost::run_scope(cx);
    }
    (keys, ready, errors)
}
pub(crate) fn prepare(handles: Option<Vec<String>>, cx: &mut App) -> StatusQuery {
    let (keys, ready, errors) = resolve(handles, cx);
    let mut runs: Vec<_> = ready.into_iter().map(StatusItem::Ready).collect();
    runs.extend(keys.into_iter().map(|key| {
        let recovered = RunsHost::run_recovered(&key, cx);
        StatusItem::Query(RunsHost::run_query(key, cx), recovered)
    }));
    StatusQuery { runs, errors }
}

impl StatusQuery {
    pub(crate) fn read(self) -> serde_json::Value {
        let runs: Vec<_> = self
            .runs
            .into_iter()
            .map(|item| match item {
                StatusItem::Query(query, recovered) => wire::RunResult {
                    recovered,
                    ..convert(query.read())
                },
                StatusItem::Ready(result) => result,
            })
            .collect();
        super::convert::tool_result(
            false,
            serde_json::json!({"runs":runs, "errors":self.errors}),
        )
    }
}

/// A run's current result: an end that can be resumed says so.
pub(super) fn convert(snapshot: domain::RunSnapshot) -> wire::RunResult {
    let resumable = snapshot.resumable();
    convert_as(snapshot, resumable)
}

/// A delivered episode: `recovered` and `resumable` are its own, not its key's.
pub(super) fn convert_episode(event: crate::runs::watch::RunEvent) -> wire::RunResult {
    let resumable = event.resumable();
    wire::RunResult {
        recovered: event.origin == crate::runs::watch::Origin::Recovered,
        ..convert_as(event.snapshot, resumable)
    }
}

/// The handle a control caller holds for `key`.
pub(crate) fn run_handle(key: domain::RunKey) -> wire::RunHandle {
    match key {
        domain::RunKey::Flow(run_id) => wire::RunHandle::Flow { run_id },
        domain::RunKey::Task(exec) => wire::RunHandle::Task {
            task_id: exec.task_id,
            execution_id: exec.execution_id,
        },
    }
}

/// The run `handle` names; `None` for a Task's latest execution, which only
/// the task book can resolve.
pub(crate) fn run_key(handle: wire::RunHandle) -> Option<domain::RunKey> {
    match handle {
        wire::RunHandle::Flow { run_id } => Some(domain::RunKey::Flow(run_id)),
        wire::RunHandle::Task {
            task_id,
            execution_id,
        } => Some(domain::RunKey::Task(daruda_store::tasks::ExecutionRef {
            task_id,
            execution_id,
        })),
        wire::RunHandle::TaskLatest { .. } => None,
    }
}

fn convert_as(snapshot: domain::RunSnapshot, resumable: bool) -> wire::RunResult {
    let handle = run_handle(snapshot.key);
    let state = match snapshot.state {
        domain::RunState::Unknown => wire::RunState::Unknown,
        domain::RunState::Gone => wire::RunState::Gone,
        domain::RunState::Unreadable { reason } => wire::RunState::Unreadable { reason },
        domain::RunState::Running {
            progress,
            waiting_on,
        } => wire::RunState::Running {
            progress: wire::Progress {
                node: progress.node,
                attempt: progress.attempt,
                elapsed_secs: progress.elapsed_secs,
                chat: progress.chat,
            },
            waiting_on: waiting_on.map(|waiting| match waiting {
                domain::WaitingOn::Question { node } => wire::WaitingOn::Question { node },
                domain::WaitingOn::Permission { tool } => wire::WaitingOn::Permission { tool },
                domain::WaitingOn::Answer { question } => wire::WaitingOn::Answer { question },
            }),
        },
        domain::RunState::Ended {
            ended_as,
            text,
            pointers,
        } => wire::RunState::Ended {
            resumable,
            ended_as: match ended_as {
                domain::EndedAs::Completed => wire::EndedAs::Completed,
                domain::EndedAs::Failed => wire::EndedAs::Failed,
                domain::EndedAs::Canceled => wire::EndedAs::Canceled,
                domain::EndedAs::Stalled => wire::EndedAs::Stalled,
                domain::EndedAs::Crashed => wire::EndedAs::Crashed,
                domain::EndedAs::Interrupted => wire::EndedAs::Interrupted,
                domain::EndedAs::Undelivered => wire::EndedAs::Undelivered,
                domain::EndedAs::Error => wire::EndedAs::Error,
                domain::EndedAs::BudgetExhausted => wire::EndedAs::BudgetExhausted,
            },
            text,
            pointers: wire::Pointers {
                run_dir: pointers.run_dir,
                report: pointers.report,
                outputs: pointers.outputs,
                chat: pointers.chat,
                transcript: pointers.transcript,
            },
        },
    };
    wire::RunResult {
        handle,
        what: snapshot.what,
        why: snapshot.why,
        state,
        recovered: false,
    }
}
#[cfg(test)]
mod tests;
