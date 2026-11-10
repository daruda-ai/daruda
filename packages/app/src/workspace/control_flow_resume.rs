//! Resuming an interrupted flow run through the control surface.
//!
//! The run is named by its id; its directory says which worktree keeps it.
//! What no answer could change is refused before the card, and checked
//! again after it, since the worktree can change while the card waits.
//! Each check is in two halves: what this process knows, on the UI thread,
//! then the run's directory, read off it.

use std::path::{Path, PathBuf};

use gpui::{App, Context};

use crate::control::result::{ControlError, FlowNotStartedReason, LaneHandle, NotResumableReason};
use crate::workspace::Workspace;
use crate::workspace::flow_ops::FlowStartRefused;
use crate::workspace::flow_paths;
use crate::workspace::flow_request::{ReadResume, ResumeSite};
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

/// A run a resume may pick up as far as this process knows, with where to
/// read it. [`Self::read`] blocks on the run's files.
pub(crate) struct ResumeRead {
    name: String,
    run_dir: PathBuf,
    site: ResumeSite,
}

impl ResumeRead {
    /// Read the run, refusing one its directory says cannot be picked up.
    pub(crate) fn read(self) -> Result<ResumableRun, ControlError> {
        let Self {
            name,
            run_dir,
            site,
        } = self;
        match site.read(&run_dir) {
            Ok(read) => Ok(ResumableRun { name, read }),
            Err(error) => Err(resume_refusal(name, error)),
        }
    }
}

/// A run read as a resume continues it.
pub(crate) struct ResumableRun {
    name: String,
    read: ReadResume,
}

impl ResumableRun {
    /// What an approval card says about resuming this run.
    pub(crate) fn plan(&self) -> FlowResumePlan {
        let resumed = &self.read.resumed;
        let selected = match &resumed.until {
            Some(until) => resumed.loaded.graph().ancestors(until).len() + 1,
            None => resumed.loaded.flow().nodes.len(),
        };
        FlowResumePlan {
            name: self.name.clone(),
            passed: resumed.passed.len().min(selected),
            total: selected,
        }
    }
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

    /// Whether the run in `run_dir`, kept by `lane`, can be picked up as far
    /// as this process knows, and where to read it to find out the rest.
    ///
    /// The worktree is checked before the run: a lock another run holds
    /// makes this run's own status unreadable, and the run in the way is the
    /// answer worth giving, named as a start names it.
    pub(crate) fn control_flow_resume_site(
        &self,
        lane: LaneRef,
        run_dir: &Path,
        cx: &App,
    ) -> Result<ResumeRead, ControlError> {
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
        match self.resume_site(lane) {
            Ok(site) => Ok(ResumeRead {
                name,
                run_dir: run_dir.to_path_buf(),
                site,
            }),
            // A worktree that no longer resolves is refused as a start
            // refuses it, before any card asks about the run.
            Err(other) => {
                Err(self.control_refusal(lane, name, FlowStartRefused::Submit(other), cx))
            }
        }
    }

    /// Resume `run` for `submitter`, the way the Flows panel does. `run` is
    /// the read made after the card, since the worktree can change while the
    /// card waits; that read is what the resume is built from.
    pub(crate) fn control_flow_resume(
        &mut self,
        lane: LaneRef,
        run_dir: &Path,
        run: ResumableRun,
        submitter: crate::runs::FlowSubmitter,
        cx: &mut Context<Self>,
    ) -> Result<FlowResumed, ControlError> {
        let ResumableRun { name, read } = run;
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
