//! `daruda_flow_resume` once routed: which run a handle names, the window
//! that keeps it, and resuming it under the one handle its owner holds.

use gpui::App;

use crate::control::result::{ControlError, ControlOutcome, ControlResult};
use crate::runs::RunsHost;
use crate::runs::result::RunKey;
use daruda_control_types::runs::RunHandle;
use daruda_store::tasks::ExecutionRef;

/// The flow run a resume picks up, and the key its end is delivered under.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResumeTarget {
    pub(super) run_id: String,
    pub(super) owner: RunKey,
}

/// What `run` names. A Task's handle names the flow run its current
/// execution runs as; a flow handle whose run a Task in the scope runs as is
/// that Task's, so the orchestrator never holds two handles for one run.
pub(super) fn target(run: &RunHandle, cx: &App) -> Result<ResumeTarget, ControlError> {
    let task_target = |exec: ExecutionRef| {
        let run_id = RunsHost::flow_run_of_task(&exec, cx).ok_or(ControlError::FlowRunNotFound)?;
        Ok(ResumeTarget {
            run_id,
            owner: RunKey::Task(exec),
        })
    };
    match run {
        RunHandle::Flow { run_id } => {
            let owner = RunsHost::task_of_flow_run(run_id, cx)
                .map(RunKey::Task)
                .filter(|task| RunsHost::run_scope(cx).contains(task))
                .unwrap_or_else(|| RunKey::Flow(run_id.clone()));
            Ok(ResumeTarget {
                run_id: run_id.clone(),
                owner,
            })
        }
        RunHandle::Task {
            task_id,
            execution_id,
        } => task_target(ExecutionRef {
            task_id: task_id.clone(),
            execution_id: execution_id.clone(),
        }),
        RunHandle::TaskLatest { task_id } => match RunsHost::latest_task_key(task_id, cx) {
            Some(RunKey::Task(exec)) => task_target(exec),
            _ => Err(ControlError::FlowRunNotFound),
        },
    }
}

/// Resume `target` as the approved command's own episode: its starter and
/// the solicitation captured before the card. An orchestrator resume scopes
/// only the owner's key; a Task's flow key stays out, or one run would end
/// twice.
pub(super) fn resume(
    target: ResumeTarget,
    origin: super::super::CommandOrigin,
    solicited: bool,
    cx: &mut App,
) -> ControlOutcome {
    let resumed = in_run_window(&target.run_id, cx, |ws, lane, run_dir, cx| {
        ws.control_flow_resume(lane, run_dir, (origin.flow_started_by(), solicited), cx)
    })?;
    if matches!(origin, super::super::CommandOrigin::Orchestrator) {
        RunsHost::register_run_solicited(target.owner.clone(), solicited, cx);
    }
    Ok(ControlResult::FlowResuming {
        handle: handle_of(target.owner),
        name: resumed.name,
        lane: resumed.lane,
    })
}

fn handle_of(key: RunKey) -> RunHandle {
    match key {
        RunKey::Flow(run_id) => RunHandle::Flow { run_id },
        RunKey::Task(exec) => RunHandle::Task {
            task_id: exec.task_id,
            execution_id: exec.execution_id,
        },
    }
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

    /// The answer's handle is the key the end arrives under.
    #[test]
    fn the_answer_names_the_owners_handle() {
        assert_eq!(
            handle_of(RunKey::Flow("run".into())),
            "flow:run".parse().unwrap()
        );
        let exec = ExecutionRef {
            task_id: "task".into(),
            execution_id: "exec".into(),
        };
        assert_eq!(
            handle_of(RunKey::Task(exec)),
            "task:task/exec".parse().unwrap()
        );
    }

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
            assert_eq!(
                target(&"flow:run".parse().unwrap(), cx),
                Ok(ResumeTarget {
                    run_id: "run".into(),
                    owner: RunKey::Flow("run".into()),
                })
            );
        });
    }
}
