//! Resuming an interrupted flow run through the control surface.
//!
//! The run is named by its id; its directory says which worktree keeps it.
//! What no answer could change is refused before the card, and checked
//! again after it, since the worktree can change while the card waits.

use std::path::Path;

use gpui::{App, Context};

use crate::control::result::{ControlError, FlowNotStartedReason, LaneHandle, NotResumableReason};
use crate::workspace::Workspace;
use crate::workspace::flow_ops::FlowStartRefused;
use crate::workspace::flow_paths;
use crate::workspace::flow_request::{FlowSubmitError, ReadResume};
use daruda_control_types::runs::RunHandle;
use daruda_flow::marker::RunStatus;
use daruda_flow::resume::ResumeError;
use daruda_store::project::LaneRef;

/// What an approval card says about a run it asks to resume.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FlowResumePlan {
    pub name: String,
    /// Nodes the run log says passed, of the nodes the run selected.
    pub passed: usize,
    pub total: usize,
}

/// A resume handed to the process, under the run's own id.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FlowResumed {
    pub name: String,
    pub lane: LaneHandle,
    pub run_id: String,
}

impl Workspace {
    /// The lane of this window whose worktree keeps `run_dir`.
    pub(crate) fn control_lane_holding_run(&self, run_dir: &Path) -> Option<LaneRef> {
        let runs = run_dir.parent()?;
        self.projects
            .lanes()
            .find(|(_, _, lane)| {
                daruda_core::path::same_path(&flow_paths::runs_dir(&lane.path), runs)
            })
            .map(|(lane_ref, _, _)| lane_ref)
    }

    /// Whether the run in `run_dir`, kept by `lane`, can be picked up now,
    /// and what a card asking about it names.
    ///
    /// The worktree is checked before the run: a lock another run holds
    /// makes this run's own status unreadable, and the run in the way is the
    /// answer worth giving, named as a start names it.
    pub(crate) fn control_flow_resume_check(
        &self,
        lane: LaneRef,
        run_dir: &Path,
        cx: &App,
    ) -> Result<FlowResumePlan, ControlError> {
        let (name, read) = self.read_resumable(lane, run_dir, cx)?;
        let resumed = &read.resumed;
        let selected = match &resumed.until {
            Some(until) => resumed.loaded.graph().ancestors(until).len() + 1,
            None => resumed.loaded.flow().nodes.len(),
        };
        Ok(FlowResumePlan {
            name,
            passed: resumed.passed.len().min(selected),
            total: selected,
        })
    }

    /// [`Self::control_flow_resume_check`]'s checks, answering the run's
    /// name and the read a resume is built from.
    fn read_resumable(
        &self,
        lane: LaneRef,
        run_dir: &Path,
        cx: &App,
    ) -> Result<(String, ReadResume), ControlError> {
        let cwd = self
            .lane_for(lane)
            .map(|lane| lane.path.clone())
            .ok_or(ControlError::FlowRunNotFound)?;
        let name = crate::runs::flow_run_name(run_dir);
        let running = |name: String| ControlError::FlowRunNotResumable {
            name,
            reason: NotResumableReason::Running,
        };
        if self
            .flow_run_in(lane, cx)
            .is_some_and(|run| daruda_core::path::same_path(run.view().run_dir(), run_dir))
        {
            return Err(running(name));
        }
        if let Some(busy) = self.lane_busy(lane, &cwd, cx) {
            let held_by = self.busy_handle(lane, busy, cx);
            // A live process holding the lock for this very run is the run
            // still going, not another run in its way.
            if matches!(&held_by, Some(RunHandle::Flow { run_id })
                if run_dir.file_name() == Some(std::ffi::OsStr::new(run_id)))
            {
                return Err(running(name));
            }
            return Err(ControlError::FlowLocked { held_by, name });
        }
        match self.read_flow_resume(lane, run_dir) {
            Ok(read) => Ok((name, read)),
            Err(FlowSubmitError::Resume(error)) => Err(resume_refusal(name, error)),
            // A worktree that no longer resolves is refused as a start
            // refuses it, before any card asks about the run.
            Err(other) => {
                Err(self.control_refusal(lane, name, FlowStartRefused::Submit(other), cx))
            }
        }
    }

    /// Resume the run in `run_dir` for `submitter`, the way the Flows panel
    /// does, after checking it again: the card may have waited while the
    /// worktree changed. That one read is what the resume is built from.
    pub(crate) fn control_flow_resume(
        &mut self,
        lane: LaneRef,
        run_dir: &Path,
        submitter: crate::runs::FlowSubmitter,
        cx: &mut Context<Self>,
    ) -> Result<FlowResumed, ControlError> {
        let (name, read) = self.read_resumable(lane, run_dir, cx)?;
        let run_id = match self.submit_flow_resume(lane, run_dir, read, submitter, cx) {
            Ok(run_id) => run_id,
            Err(refused) => return Err(self.control_refusal(lane, name, refused, cx)),
        };
        Ok(FlowResumed {
            name,
            lane: LaneHandle::new(self.uuid(), lane),
            run_id,
        })
    }
}

/// The engine's refusal to resume, as a control caller reads it. A spec that
/// no longer loads is the start refusal it always was; the rest say why the
/// run is not one to continue.
fn resume_refusal(name: String, error: ResumeError) -> ControlError {
    let reason = match error {
        ResumeError::NotResumable(status) => not_resumable(status),
        ResumeError::NothingStarted => NotResumableReason::NeverStarted,
        ResumeError::Spec(_) | ResumeError::Load(_) => {
            return ControlError::FlowNotStarted {
                name,
                reason: FlowNotStartedReason::ResumeRefused,
            };
        }
    };
    ControlError::FlowRunNotResumable { name, reason }
}

/// Why a status the engine will not resume is not resumable.
fn not_resumable(status: RunStatus) -> NotResumableReason {
    match status {
        RunStatus::Running => NotResumableReason::Running,
        RunStatus::Done | RunStatus::Failed | RunStatus::Canceled => NotResumableReason::Ended,
        RunStatus::Crashed | RunStatus::Stalled | RunStatus::Unknown => NotResumableReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every status the engine refuses maps to the reason a caller acts on;
    /// the resumable two cannot reach here, and read as unknown if they did.
    #[test]
    fn each_refused_status_names_why() {
        assert_eq!(
            not_resumable(RunStatus::Running),
            NotResumableReason::Running
        );
        for ended in [RunStatus::Done, RunStatus::Failed, RunStatus::Canceled] {
            assert_eq!(not_resumable(ended), NotResumableReason::Ended, "{ended:?}");
        }
        assert_eq!(
            not_resumable(RunStatus::Unknown),
            NotResumableReason::Unknown
        );
    }

    #[test]
    fn a_run_with_no_record_is_started_again_and_a_bad_spec_is_a_start_refusal() {
        assert_eq!(
            resume_refusal("ship".into(), ResumeError::NothingStarted),
            ControlError::FlowRunNotResumable {
                name: "ship".into(),
                reason: NotResumableReason::NeverStarted,
            }
        );
        assert_eq!(
            resume_refusal("ship".into(), ResumeError::Spec("gone".into())),
            ControlError::FlowNotStarted {
                name: "ship".into(),
                reason: FlowNotStartedReason::ResumeRefused,
            }
        );
    }
}
