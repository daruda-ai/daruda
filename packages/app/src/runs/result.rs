//! Execution results owned by the process, independent of transport types.
use daruda_control_types::agent_text::bound_agent_text;
use daruda_core::text::char_prefix;
use daruda_store::tasks::{ExecutionRef, Lifecycle, TaskExecution, TaskRecord};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RunKey {
    Flow(String),
    Task(ExecutionRef),
}
impl RunKey {
    /// A flow run's key: its run directory's name, the run id handles carry.
    pub(crate) fn flow_of(run_dir: &std::path::Path) -> Self {
        Self::Flow(flow_run_id(run_dir))
    }
}
pub(super) fn flow_run_id(run_dir: &std::path::Path) -> String {
    run_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
/// Flows before Tasks, each by id: a stable order for listing a scope.
impl Ord for RunKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Flow(a), Self::Flow(b)) => a.cmp(b),
            (Self::Task(a), Self::Task(b)) => {
                (&a.task_id, &a.execution_id).cmp(&(&b.task_id, &b.execution_id))
            }
            (Self::Flow(_), Self::Task(_)) => std::cmp::Ordering::Less,
            (Self::Task(_), Self::Flow(_)) => std::cmp::Ordering::Greater,
        }
    }
}
impl PartialOrd for RunKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunSnapshot {
    pub key: RunKey,
    pub what: String,
    pub why: Option<String>,
    pub state: RunState,
}

impl RunSnapshot {
    /// Whether this is an end a resume picks up.
    pub(crate) fn resumable(&self) -> bool {
        matches!(self.state, RunState::Ended { ended_as, .. } if ended_as.resumable())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunState {
    Running {
        progress: Progress,
        waiting_on: Option<WaitingOn>,
    },
    Ended {
        ended_as: EndedAs,
        text: Option<String>,
        pointers: Pointers,
    },
    Unknown,
    Gone,
    Unreadable {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EndedAs {
    Completed,
    Failed,
    Canceled,
    Stalled,
    Crashed,
    Interrupted,
    Undelivered,
    Error,
    BudgetExhausted,
}

impl EndedAs {
    /// A flow run that stopped without deciding to, which a resume picks up:
    /// the ends `daruda_flow::resume::is_resumable` accepts.
    pub(crate) fn resumable(self) -> bool {
        matches!(self, Self::Crashed | Self::Stalled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum WaitingOn {
    Question { node: String },
    Permission { tool: Option<String> },
    Answer { question: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Progress {
    pub node: Option<String>,
    pub attempt: Option<u32>,
    pub elapsed_secs: u64,
    pub chat: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pointers {
    pub run_dir: Option<std::path::PathBuf>,
    pub report: Option<std::path::PathBuf>,
    pub outputs: Vec<std::path::PathBuf>,
    pub chat: Option<String>,
    pub transcript: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct FlowRunLocation {
    pub run_dir: std::path::PathBuf,
    pub lock_dir: Option<std::path::PathBuf>,
}

const WHY_CHARS: usize = 300;

pub(super) fn task_snapshot(record: &TaskRecord, exec: &ExecutionRef) -> Option<RunSnapshot> {
    let ended = record.ended_executions.iter().find(|end| &end.exec == exec);
    let (lifecycle, execution, started_at, text) = if let Some(end) = ended {
        (
            &end.ended_as,
            end.execution.as_ref(),
            end.started_at,
            end.result.as_deref(),
        )
    } else if record.current_exec() == Some(exec) {
        (
            &record.lifecycle,
            record.execution.as_ref(),
            record.started_at,
            None,
        )
    } else {
        return None;
    };
    let description = ended.map_or(record.description.as_ref(), |end| end.description.as_ref());
    let pointers = task_pointers(execution);
    let ended_as = match lifecycle {
        Lifecycle::Done { .. } => Some(EndedAs::Completed),
        Lifecycle::Error { .. } => Some(EndedAs::Error),
        Lifecycle::Cancelled { .. } => Some(EndedAs::Canceled),
        Lifecycle::Undelivered { .. } => Some(EndedAs::Undelivered),
        Lifecycle::Interrupted { .. } => Some(EndedAs::Interrupted),
        _ => None,
    };
    let state = if let Some(ended_as) = ended_as {
        RunState::Ended {
            ended_as,
            text: text.and_then(bound_agent_text),
            pointers,
        }
    } else {
        RunState::Running {
            progress: Progress {
                elapsed_secs: started_at
                    .map(|at| (chrono::Utc::now() - at).num_seconds().max(0) as u64)
                    .unwrap_or(0),
                chat: pointers.chat.clone(),
                ..Default::default()
            },
            waiting_on: None,
        }
    };
    Some(RunSnapshot {
        key: RunKey::Task(exec.clone()),
        what: description
            .map(|description| description.title.clone())
            .unwrap_or_default(),
        why: description
            .map(|description| char_prefix(&description.prompt, WHY_CHARS).0.to_owned())
            .filter(|v| !v.trim().is_empty()),
        state,
    })
}

fn task_pointers(execution: Option<&TaskExecution>) -> Pointers {
    let run_dir = execution.and_then(super::flow_tasks::task_run_dir);
    Pointers {
        chat: execution.and_then(|run| run.session_id.clone()),
        transcript: execution.and_then(|run| match &run.source {
            daruda_store::tasks::TaskExecutionSource::ClaudeCli {
                transcript_path, ..
            } => transcript_path.clone(),
            daruda_store::tasks::TaskExecutionSource::AgentChat
            | daruda_store::tasks::TaskExecutionSource::Flow { .. } => None,
        }),
        report: run_dir
            .as_ref()
            .map(|dir| dir.join(daruda_flow::record::RUN_REPORT_FILE)),
        run_dir,
        ..Default::default()
    }
}

/// Whether `snapshot` is a Task's flow interruption: the end [`through_flow`]
/// gives a still-running Task, whose own end is a later episode.
pub(super) fn is_interruption(snapshot: &RunSnapshot) -> bool {
    matches!(snapshot.key, RunKey::Task(_)) && snapshot.resumable()
}

/// A running task's result, read through its flow run: a flow interrupted
/// where it can be resumed is the task's end until a resume reopens it. The
/// task itself stays running, so the person's resume settles it later.
pub(super) fn through_flow(mut task: RunSnapshot, flow: RunSnapshot) -> RunSnapshot {
    if matches!(task.state, RunState::Running { .. })
        && matches!(flow.state, RunState::Ended { ended_as, .. } if ended_as.resumable())
    {
        task.state = flow.state;
    }
    task
}

/// Freeze the end from process facts even when the log/report could not be written.
pub(super) fn flow_ended(
    mut snapshot: RunSnapshot,
    end: &daruda_flow::event::RunEnd,
    pointers: Pointers,
) -> RunSnapshot {
    use daruda_flow::event::RunEnd;
    let ended_as = match end {
        RunEnd::Done => EndedAs::Completed,
        RunEnd::Failed { .. } => EndedAs::Failed,
        RunEnd::Canceled { .. } => EndedAs::Canceled,
        RunEnd::BudgetExhausted { .. } => EndedAs::BudgetExhausted,
        RunEnd::Stalled { .. } => EndedAs::Stalled,
        RunEnd::Io { .. }
        | RunEnd::LockHeld { .. }
        | RunEnd::Invalid { .. }
        | RunEnd::Unprovisioned { .. }
        | RunEnd::ShutDown
        | RunEnd::Log { .. }
        | RunEnd::NotResumed { .. } => EndedAs::Error,
    };
    let text = match end {
        RunEnd::Io { message, .. }
        | RunEnd::Log { message, .. }
        | RunEnd::NotResumed { message }
        | RunEnd::Unprovisioned { message, .. } => bound_agent_text(message),
        _ => None,
    };
    snapshot.state = RunState::Ended {
        ended_as,
        text,
        pointers,
    };
    snapshot
}

pub(super) fn flow_why(
    until: Option<&daruda_flow::NodeId>,
    pinned: impl Iterator<Item = String>,
) -> Option<String> {
    let mut why = Vec::new();
    if let Some(until) = until {
        why.push(format!("until: {until}"));
    }
    let pinned = pinned.collect::<Vec<_>>();
    if !pinned.is_empty() {
        why.push(format!("pinned: {}", pinned.join(", ")));
    }
    (!why.is_empty()).then(|| why.join("; "))
}

#[cfg(test)]
mod tests;
