//! `daruda_flow_resume` once routed: which run a handle names, the window
//! that keeps it, and resuming it under the one handle its owner holds.

use gpui::App;

use crate::control::mcp::runs::{run_handle, run_key};
use crate::control::result::{ControlError, ControlOutcome, ControlResult, NotResumableReason};
use crate::runs::RunsHost;
use crate::runs::result::RunKey;
use crate::surface::strings as s;
use daruda_control_types::runs::RunHandle;
use daruda_store::tasks::ExecutionRef;

/// The flow run a resume picks up, resolved once before the card so the
/// approved work is the run the card named.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResumeTarget {
    pub(super) run_id: String,
    named: Named,
}

/// What the command named the run by.
#[derive(Debug, PartialEq, Eq)]
enum Named {
    Flow,
    /// The task execution the run is the flow of.
    Task(ExecutionRef),
}

/// What `run` names. A Task's handle names the flow run its current
/// execution runs as, while that Task still runs: a Task that ended is not
/// settled by its run's end, so it cannot own a resume.
pub(super) fn target(run: &RunHandle, cx: &App) -> Result<ResumeTarget, ControlError> {
    let task_target = |exec: ExecutionRef| {
        Ok(ResumeTarget {
            run_id: running_task_run(&exec, cx)?,
            named: Named::Task(exec),
        })
    };
    let key = match run {
        RunHandle::TaskLatest { task_id } => RunsHost::latest_task_key(task_id, cx),
        concrete => run_key(concrete.clone()),
    };
    match key {
        Some(RunKey::Flow(run_id)) => Ok(ResumeTarget {
            run_id,
            named: Named::Flow,
        }),
        Some(RunKey::Task(exec)) => task_target(exec),
        None => Err(ControlError::FlowRunNotFound),
    }
}

impl ResumeTarget {
    /// The one key the resumed run's end arrives under, against the scope as
    /// it stands now. A Task named by the command that stopped running is
    /// refused: its run's end would settle nothing.
    pub(super) fn owner(&self, cx: &App) -> Result<RunKey, ControlError> {
        let task = match &self.named {
            Named::Flow => RunsHost::running_task_of_flow_run(&self.run_id, cx),
            Named::Task(exec) => {
                running_task_run(exec, cx)?;
                Some(exec.clone())
            }
        };
        Ok(owner(
            RunKey::Flow(self.run_id.clone()),
            task.map(RunKey::Task),
            &self.named,
            &RunsHost::run_scope(cx),
        ))
    }
}

/// One run, one key. A key already in the scope owns the run, the running
/// Task's first, so the other never joins it; with neither scoped, the key
/// the command named does.
fn owner(flow: RunKey, task: Option<RunKey>, named: &Named, scope: &[RunKey]) -> RunKey {
    let Some(task) = task else {
        return flow;
    };
    if scope.contains(&task) {
        task
    } else if scope.contains(&flow) {
        flow
    } else {
        match named {
            Named::Task(_) => task,
            Named::Flow => flow,
        }
    }
}

/// The flow run `exec` runs as, while its task still runs.
fn running_task_run(exec: &ExecutionRef, cx: &App) -> Result<String, ControlError> {
    let run = RunsHost::flow_run_of_task(exec, cx).ok_or(ControlError::FlowRunNotFound)?;
    if !run.running {
        return Err(ControlError::FlowRunNotResumable {
            name: crate::runs::flow_run_name(&run.run_dir),
            reason: NotResumableReason::Ended,
        });
    }
    Ok(run.run_id)
}

/// What the card says about resuming `target`: the run is read here, and
/// anything no answer could change is refused before the card.
pub(super) fn summary(target: &ResumeTarget, cx: &mut App) -> Result<String, ControlError> {
    let plan = in_run_window(&target.run_id, cx, |ws, lane, run_dir, cx| {
        ws.control_flow_resume_check(lane, run_dir, cx)
    })?;
    Ok(if plan.passed == plan.total {
        s::control::approval_flow_resume_all_passed(&plan.name, plan.total)
    } else {
        s::control::approval_flow_resume(&plan.name, plan.passed, plan.total)
    })
}

/// Resume `target` as the approved command's own episode: its starter and
/// the solicitation captured before the card. An orchestrator resume scopes
/// only the owner's key, or one run would end twice.
pub(super) fn resume(
    target: ResumeTarget,
    origin: super::super::CommandOrigin,
    solicited: bool,
    cx: &mut App,
) -> ControlOutcome {
    let owner = target.owner(cx)?;
    let resumed = in_run_window(&target.run_id, cx, |ws, lane, run_dir, cx| {
        ws.control_flow_resume(lane, run_dir, (origin.flow_started_by(), solicited), cx)
    })?;
    if matches!(origin, super::super::CommandOrigin::Orchestrator) {
        RunsHost::register_run_solicited(owner.clone(), solicited, cx);
    }
    Ok(ControlResult::FlowResuming {
        handle: run_handle(owner),
        name: resumed.name,
        lane: resumed.lane,
    })
}

/// Run `f` in the window whose worktree keeps flow run `run_id`. The run is
/// found where a result query finds it, so a handle that reads as a result
/// names the run a resume picks up.
pub(super) fn in_run_window<T>(
    run_id: &str,
    cx: &mut App,
    mut f: impl FnMut(
        &mut crate::workspace::Workspace,
        daruda_store::project::LaneRef,
        &std::path::Path,
        &mut gpui::Context<crate::workspace::Workspace>,
    ) -> Result<T, ControlError>,
) -> Result<T, ControlError> {
    let run_dir = RunsHost::flow_location(run_id, cx)
        .map(|location| location.run_dir)
        .filter(|run_dir| run_dir.is_dir())
        .ok_or(ControlError::FlowRunNotFound)?;
    let mut out = None;
    crate::window_registry::WindowRegistry::for_each_workspace(cx, |ws, _window, cx| {
        if out.is_none()
            && let Some(lane) = ws.control_lane_holding_run(&run_dir)
        {
            out = Some(f(ws, lane, &run_dir, cx));
        }
    });
    out.unwrap_or(Err(ControlError::FlowRunNotFound))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_handle_naming_no_flow_run_is_not_found(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            RunsHost::ensure_installed(cx);
            for handle in ["task:missing/exec", "task:missing"] {
                assert_eq!(
                    target(&handle.parse().unwrap(), cx),
                    Err(ControlError::FlowRunNotFound),
                    "{handle}"
                );
            }
            let target = target(&"flow:run".parse().unwrap(), cx).unwrap();
            assert_eq!(target.run_id, "run");
            assert_eq!(target.owner(cx), Ok(RunKey::Flow("run".into())));
        });
    }

    /// Whichever key is already scoped keeps the run; the Task's first.
    #[test]
    fn a_scoped_key_owns_the_run_before_the_one_named() {
        let exec = ExecutionRef {
            task_id: "task".into(),
            execution_id: "exec".into(),
        };
        let flow = RunKey::Flow("run".into());
        let task = RunKey::Task(exec.clone());
        let by_task = Named::Task(exec);
        let owns = |task_key: Option<&RunKey>, named: &Named, scope: &[&RunKey]| {
            let scope: Vec<RunKey> = scope.iter().map(|key| (*key).clone()).collect();
            owner(flow.clone(), task_key.cloned(), named, &scope)
        };
        assert_eq!(owns(None, &Named::Flow, &[]), flow, "no running task");
        assert_eq!(owns(Some(&task), &Named::Flow, &[]), flow);
        assert_eq!(owns(Some(&task), &by_task, &[]), task);
        assert_eq!(owns(Some(&task), &Named::Flow, &[&task]), task);
        assert_eq!(owns(Some(&task), &by_task, &[&flow]), flow);
        assert_eq!(owns(Some(&task), &Named::Flow, &[&flow, &task]), task);
    }
}
