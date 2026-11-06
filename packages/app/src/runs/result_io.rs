//! Read-only enrichment of result snapshots, used off the UI thread by queries.
use super::flow_description::{FlowDescription, StartFacts};
use super::result::{self, EndedAs, Pointers, RunKey, RunSnapshot, RunState};
use daruda_control_types::agent_text::bound_agent_text;
use std::path::Path;

pub(super) fn enrich(mut snapshot: RunSnapshot) -> RunSnapshot {
    if let RunState::Ended { text, pointers, .. } = &mut snapshot.state
        && let Some(report) = pointers.report.as_deref().and_then(read_report)
    {
        *text = Some(report);
    }
    snapshot
}

/// A run's `run.md` as an agent may read it; `None` when it cannot be read.
pub(super) fn read_report(report: &Path) -> Option<String> {
    let text = std::fs::read_to_string(report).ok()?;
    bound_agent_text(&text)
}

#[cfg(test)]
pub(super) fn flow_disk(location: &super::result::FlowRunLocation) -> RunSnapshot {
    flow_observation(location).snapshot
}

pub(super) struct FlowObservation {
    pub snapshot: RunSnapshot,
    pub resumed_at: Option<u64>,
}

pub(super) fn flow_observation(location: &super::result::FlowRunLocation) -> FlowObservation {
    let dir = &location.run_dir;
    let mut snapshot = RunSnapshot {
        key: RunKey::flow_of(dir),
        what: result::flow_run_id(dir),
        why: None,
        state: RunState::Unknown,
    };
    if !dir.exists() {
        snapshot.state = RunState::Gone;
        return FlowObservation {
            snapshot,
            resumed_at: None,
        };
    }
    // A legacy journal records no source and is read for no end: the
    // status markers say how it ended.
    let (resumed_at, source, replay, end) = match daruda_flow::run_log::reader::inspect(dir) {
        Err(error) => {
            snapshot.what = FlowDescription::read(dir, StartFacts::default()).what;
            snapshot.state = RunState::Unreadable {
                reason: error.to_string(),
            };
            return FlowObservation {
                snapshot,
                resumed_at: None,
            };
        }
        Ok(Some(read)) => (
            read.resumed_at,
            read.state.source().map(Path::to_path_buf),
            read.replay,
            read.end,
        ),
        Ok(None) => (0, None, daruda_flow::run_log::reader::read(dir), None),
    };
    let description = FlowDescription::read(
        dir,
        StartFacts {
            source: source.as_deref(),
            until: replay.until.as_ref(),
            pinned: replay.pinned.iter().collect(),
        },
    );
    snapshot.what = description.what;
    snapshot.why = description.why;
    snapshot = match end {
        Some(end) => result::flow_ended(snapshot, &end, description.pointers),
        None => {
            snapshot.state = open_state(
                dir,
                location.lock_dir.as_deref(),
                description.pointers,
                replay.passed.len(),
            );
            snapshot
        }
    };
    FlowObservation {
        snapshot: enrich(snapshot),
        resumed_at: Some(resumed_at),
    }
}

/// Fixed English like the notice protocol: the reader is the orchestrator.
const STOPPED_TEXT: &str = "The run stopped before it ended: daruda quit or its process died";
const STALLED_TEXT: &str = "The run stalled: nothing left in it could start";

/// `passed` is how many nodes the run log says passed, for a run that may
/// have stopped without writing its report.
fn open_state(dir: &Path, lock_dir: Option<&Path>, pointers: Pointers, passed: usize) -> RunState {
    use daruda_flow::marker::RunStatus;
    let status = daruda_flow::marker::run_status(dir, lock_dir, &daruda_core::process::is_alive);
    match ended_as_of(status) {
        Some(ended_as) => RunState::Ended {
            ended_as,
            text: resumable_text(ended_as, passed),
            pointers,
        },
        None if status == RunStatus::Running => RunState::Running {
            progress: Default::default(),
            waiting_on: None,
        },
        None => RunState::Unknown,
    }
}

/// What an orchestrator reads for an end it can resume, until a report
/// replaces it; `None` for any other end.
fn resumable_text(ended_as: EndedAs, passed: usize) -> Option<String> {
    let why = match ended_as {
        EndedAs::Crashed => STOPPED_TEXT,
        EndedAs::Stalled => STALLED_TEXT,
        _ => return None,
    };
    let nodes = if passed == 1 { "node" } else { "nodes" };
    Some(format!(
        "{why}. {passed} {nodes} passed. It can be resumed from where it stopped."
    ))
}

/// The end a run directory's status names; `None` while it may still run.
fn ended_as_of(status: daruda_flow::marker::RunStatus) -> Option<EndedAs> {
    use daruda_flow::marker::RunStatus;
    match status {
        RunStatus::Running | RunStatus::Unknown => None,
        RunStatus::Done => Some(EndedAs::Completed),
        RunStatus::Failed => Some(EndedAs::Failed),
        RunStatus::Canceled => Some(EndedAs::Canceled),
        RunStatus::Stalled => Some(EndedAs::Stalled),
        RunStatus::Crashed => Some(EndedAs::Crashed),
    }
}

#[cfg(test)]
mod tests;
