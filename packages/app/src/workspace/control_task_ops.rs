//! Tasks on behalf of the control surface.
//!
//! The start and open paths are the Tasks UI's own (`begin_task_start`,
//! `begin_task_chat_open`); this file only names what they answer in the
//! control vocabulary. That translation lives inside `crate::workspace`
//! because the answers' types do.

use gpui::{Context, Window};

use daruda_store::project::{LaneRef, ProjectId, ProjectUuid};
use daruda_store::tasks::{ExecutionRef, Lifecycle, TaskSpec};

use crate::control::result::{ControlError, TaskProject};
use crate::control::spec::NewTask;
use crate::runs::{GlobalTasks, TaskCommitError, TaskIntent};
use crate::workspace::Workspace;
use crate::workspace::control_lane_ops::Unregistered;
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::right_dock::task_chat_ops::TaskChatError;
use crate::workspace::right_dock::task_start::{
    KeptWorktree, ShownTaskStart, TaskStartError, TaskStarted,
};
use crate::workspace::task_draft::{DraftError, DraftLocation, TaskDraft};

/// A started execution, its worktree, and its chat when it has one.
pub(crate) type ControlTaskStart = Result<(ExecutionRef, LaneRef, Option<PaneId>), ControlError>;

impl Workspace {
    /// Each open project's durable id, with the handle a caller names it by.
    pub(crate) fn control_task_projects(&self) -> Vec<(ProjectUuid, TaskProject)> {
        self.projects
            .iter()
            .map(|p| {
                (
                    p.uuid,
                    TaskProject {
                        workspace: self.uuid(),
                        project: p.id,
                        name: p.name.clone(),
                    },
                )
            })
            .collect()
    }

    pub(crate) fn control_has_project_uuid(&self, uuid: ProjectUuid) -> bool {
        self.project_by_uuid(uuid).is_some()
    }

    /// The spec `daruda_task_create` would add, checked as the Task editor
    /// checks its own: the same branch rules and the same project's lanes.
    /// Pure, so the approval guard can ask before the card is shown.
    pub(crate) fn control_task_spec(
        &self,
        project: ProjectId,
        request: NewTask,
    ) -> Result<TaskSpec, ControlError> {
        let uuid = self
            .project_for(project)
            .map(|p| p.uuid)
            .ok_or(ControlError::TargetGone)?;
        // A worktree of another project would leave the task pointing at a
        // repository it does not belong to.
        let location = match request.worktree.map(|lane| lane.lane_ref()) {
            None => DraftLocation::NewWorktree { base: None },
            Some(lane) if lane.project == project => DraftLocation::ExistingLane(
                self.lane_for(lane)
                    .map(|l| l.path.clone())
                    .ok_or(ControlError::TargetGone)?,
            ),
            Some(_) => return Err(ControlError::TargetGone),
        };
        let draft = TaskDraft {
            title: request.title,
            prompt: request.prompt,
            notes: request.notes.unwrap_or_default(),
            branch: request.branch.unwrap_or_default(),
            location,
            flow: request.flow.clone(),
        };
        self.task_spec_from(uuid, draft).map_err(|e| match e {
            DraftError::Flow(problem) => flow_problem_answer(request.flow.as_ref(), problem),
            DraftError::TitleEmpty => ControlError::TaskTitleEmpty,
            DraftError::ProjectNotOpen | DraftError::LaneNotInProject => ControlError::TargetGone,
            DraftError::BranchInvalid => ControlError::LaneNameInvalid,
            DraftError::BranchTaken => ControlError::TaskBranchTaken,
        })
    }

    /// How the approval for a new task names the flow it would run as, and
    /// whether that flow would be given the task's prompt.
    pub(crate) fn control_new_task_flow_approval(
        &self,
        project: ProjectId,
        worktree: Option<LaneRef>,
        flow: &daruda_store::tasks::TaskFlow,
    ) -> super::TaskFlowApproval {
        let run_in = match worktree.and_then(|lane| self.lane_for(lane)) {
            Some(lane) => daruda_store::tasks::TaskRunIn::ExistingLane {
                path: lane.path.clone(),
            },
            None => daruda_store::tasks::TaskRunIn::NewWorktree,
        };
        match self.project_for(project) {
            Some(p) => self.task_flow_approval(p.uuid, &run_in, None, flow),
            None => super::TaskFlowApproval {
                summary: crate::surface::strings::control::approval_task_as_flow_unresolved(
                    crate::control::exec::quoted_title(&flow.name),
                ),
                takes_prompt: true,
            },
        }
    }

    /// Where the approval to start `task` says it runs: the branch its
    /// first prompt's header names, quoted since the agent receives it, and
    /// the worktree an existing one runs in, or the one a new worktree
    /// branches from, which decides the code it starts with.
    pub(crate) fn control_task_start_location(
        &self,
        task: &daruda_store::tasks::TaskRecord,
    ) -> String {
        use crate::control::exec::quoted_title;
        use crate::surface::strings::control as s;
        let path = match &task.spec.run_in {
            daruda_store::tasks::TaskRunIn::NewWorktree => {
                let branch = quoted_title(&task.spec.branch_name);
                return match task.spec.base_worktree_path.as_deref() {
                    Some(base) => s::approval_task_in_new_branch_from(
                        branch,
                        quoted_title(&self.control_worktree_name_at(base)),
                    ),
                    None => s::approval_task_in_new_branch(branch),
                };
            }
            daruda_store::tasks::TaskRunIn::ExistingLane { path } => path,
        };
        // A lane gone by now refuses the start; its path is all there is.
        let Some(lane) = self.lane_ref_at(path).and_then(|lane| self.lane_for(lane)) else {
            return s::approval_task_in_worktree(quoted_title(&path.display().to_string()));
        };
        // A worktree named by its branch is named once.
        let name = lane.display_name();
        match super::right_dock::task_existing_lane_ops::lane_branch(lane) {
            Some(branch) if branch != name => {
                s::approval_task_in_worktree_on_branch(quoted_title(&name), quoted_title(branch))
            }
            _ => s::approval_task_in_worktree(quoted_title(&name)),
        }
    }

    /// The worktree at `path` by its name, or by the path once no lane
    /// holds it.
    fn control_worktree_name_at(&self, path: &std::path::Path) -> String {
        self.lane_ref_at(path)
            .and_then(|lane| self.lane_for(lane))
            .map_or_else(|| path.display().to_string(), |lane| lane.display_name())
    }

    /// How the approval to start `task` names the flow it runs as, if it
    /// runs as one, and whether that flow is given the task's prompt.
    pub(crate) fn control_task_flow_approval(
        &self,
        task: &daruda_store::tasks::TaskRecord,
    ) -> Option<super::TaskFlowApproval> {
        let spec = &task.spec;
        let flow = spec.flow.as_ref()?;
        Some(self.task_flow_approval(
            spec.project,
            &spec.run_in,
            spec.base_worktree_path.as_deref(),
            flow,
        ))
    }

    /// Add a Backlog task to `project`, saved like one the form creates.
    /// Answers once the task is in the log, so the caller's next command
    /// finds it.
    pub(crate) fn control_task_create(
        &mut self,
        project: ProjectId,
        request: NewTask,
        cx: &mut Context<Self>,
    ) -> Result<gpui::Task<Result<String, ControlError>>, ControlError> {
        let spec = self.control_task_spec(project, request)?;
        let id = spec.id.clone();
        let created = self.propose_task_command(TaskIntent::Create { spec }, cx);
        Ok(cx.spawn(async move |_, _| created.await.map(|_| id).map_err(control_commit_error)))
    }

    /// Cancel a task that is starting or running, as the Tasks UI's Cancel
    /// does. Answers once the cancel is in the log.
    pub(crate) fn control_task_stop(
        &mut self,
        task: &str,
        cx: &mut Context<Self>,
    ) -> Result<gpui::Task<Result<(), ControlError>>, ControlError> {
        let exec = match cx.global::<GlobalTasks>().get(task).map(|t| &t.lifecycle) {
            None => return Err(ControlError::TaskNotFound),
            Some(
                Lifecycle::Preparing { exec, .. }
                | Lifecycle::Dispatching { exec, .. }
                | Lifecycle::Running { exec, .. },
            ) => exec.clone(),
            Some(_) => return Err(ControlError::TaskNotRunning),
        };
        let cancelled = self.cancel_execution(exec, cx);
        Ok(cx.spawn(async move |_, _| {
            cancelled.await.map(|_| ()).map_err(|e| match e {
                TaskCommitError::Rejected(_) => ControlError::TaskNotRunning,
                other => control_commit_error(other),
            })
        }))
    }

    /// Start `task`, answering on the returned channel. A refusal is on it
    /// before this returns, as with [`Self::begin_task_start_for`], which
    /// `agent` and `shown` are passed to.
    pub(crate) fn control_task_start(
        &mut self,
        task: &str,
        submitter: crate::runs::FlowSubmitter,
        agent: crate::workspace::ChatAgent,
        shown: Option<ShownTaskStart>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> smol::channel::Receiver<ControlTaskStart> {
        let (tx, rx) = smol::channel::bounded(1);
        let started = self.begin_task_start_for(task, submitter, agent, shown, window, cx);
        // SILENT-OK on each send below: a caller that stopped listening
        // has nothing left to tell.
        if let Ok(outcome) = started.try_recv() {
            let _ = tx.try_send(self.control_task_started(outcome));
            return rx;
        }
        // Captured now: a failure that left a worktree on disk is answered
        // with it even if this window is gone by then.
        let workspace = self.uuid();
        cx.spawn(async move |ws, cx| {
            let outcome = started
                .recv()
                .await
                .unwrap_or(Err(TaskStartError::WindowClosed))
                .map_err(|e| control_start_error(e, workspace));
            let answer = match outcome {
                Ok(started) => ws
                    .update(cx, |ws, _| ws.control_task_located(started))
                    .unwrap_or(Err(ControlError::TargetGone)),
                Err(refused) => Err(refused),
            };
            let _ = tx.try_send(answer);
        })
        .detach();
        rx
    }

    fn control_task_started(
        &self,
        outcome: Result<TaskStarted, TaskStartError>,
    ) -> ControlTaskStart {
        let started = outcome.map_err(|e| control_start_error(e, self.uuid()))?;
        self.control_task_located(started)
    }

    /// The lane and chat a started task runs in.
    fn control_task_located(&self, started: TaskStarted) -> ControlTaskStart {
        let lane = self
            .lane_ref_at(&started.worktree)
            .ok_or(ControlError::TargetGone)?;
        let chat = started.chat();
        Ok((started.exec, lane, chat))
    }

    /// Bring up the chat `task`'s run belongs to; one it reopens in a new
    /// pane runs with its mode from `mode`.
    pub(crate) fn control_task_open(
        &mut self,
        task: &str,
        mode: daruda_config::ModeChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<PaneId, ControlError> {
        self.begin_task_chat_open(task, mode, window, cx)
            .map_err(|e| match e {
                TaskChatError::NotFound => ControlError::TaskNotFound,
                TaskChatError::NoSession => ControlError::TaskNoSession,
                TaskChatError::AgentUnavailable => ControlError::TaskAgentUnavailable,
                TaskChatError::WorktreeMissing => ControlError::TargetGone,
            })
    }
}

/// A task change the log did not take, in the control vocabulary.
fn control_commit_error(e: TaskCommitError) -> ControlError {
    ControlError::TaskNotRecorded {
        detail: e.to_string(),
    }
}

/// Why a started task's prompt is missing, for the model reading the answer;
/// the task's row says it in the person's language.
const PROMPT_UNDELIVERED: &str = "the prompt could not reach the pane, which closed";

/// A start failure in the control vocabulary. The detail strings are for
/// the model reading the result, not for a person, so they stay English.
/// `workspace` qualifies a worktree the failure left behind.
fn control_start_error(
    e: TaskStartError,
    workspace: daruda_store::project::WorkspaceUuid,
) -> ControlError {
    match e {
        TaskStartError::RemoteLane => ControlError::TaskStartFailed {
            detail: daruda_store::tasks::TaskRejected::RemoteLane.to_string(),
        },
        TaskStartError::NotFound => ControlError::TaskNotFound,
        TaskStartError::NotBacklog => ControlError::TaskNotBacklog,
        TaskStartError::ProjectNotOpen => ControlError::TaskProjectNotOpen,
        TaskStartError::RepoBusy => ControlError::LaneCreateBusy,
        TaskStartError::LaneMissing { .. } | TaskStartError::WindowClosed => {
            ControlError::TargetGone
        }
        TaskStartError::NoGitRepo => ControlError::TaskStartFailed {
            detail: "the project is not a git repository".into(),
        },
        TaskStartError::PaneUnavailable => ControlError::TaskStartFailed {
            detail: "no pane could be opened in the worktree".into(),
        },
        TaskStartError::PromptUndelivered => ControlError::TaskStartFailed {
            detail: PROMPT_UNDELIVERED.into(),
        },
        TaskStartError::GitAddFailed { detail } => ControlError::TaskStartFailed { detail },
        TaskStartError::NotRecorded { detail } => ControlError::TaskNotRecorded { detail },
        TaskStartError::FlowMissing => ControlError::TaskStartFailed {
            detail: crate::workspace::task_flow::TaskFlowProblem::NotFound.to_string(),
        },
        TaskStartError::FlowRefused { detail, .. } => ControlError::TaskStartFailed { detail },
        TaskStartError::AgentUnavailable => ControlError::AgentUnavailable,
        TaskStartError::ChangedSinceShown => ControlError::ApprovalStale,
        TaskStartError::WorktreeKept {
            at: KeptWorktree::Unregistered(path),
            cause,
        } => match *cause {
            TaskStartError::ProjectNotOpen => Unregistered::ProjectClosed.at(path),
            TaskStartError::WindowClosed => Unregistered::WindowClosed.at(path),
            other => ControlError::WorktreeUnregistered {
                path,
                detail: control_start_error(other, workspace).to_string(),
            },
        },
        TaskStartError::WorktreeKept {
            at: KeptWorktree::Lane(lane),
            cause,
        } => {
            let worktree = crate::control::result::LaneHandle::new(workspace, lane);
            match control_start_error(*cause, workspace) {
                ControlError::AgentUnavailable => {
                    ControlError::AgentUnavailableWorktreeKept { worktree }
                }
                ControlError::TaskStartFailed { detail }
                | ControlError::TaskNotRecorded { detail } => {
                    ControlError::TaskStartFailedWorktreeKept { worktree, detail }
                }
                other => ControlError::TaskStartFailedWorktreeKept {
                    worktree,
                    detail: other.to_string(),
                },
            }
        }
    }
}

/// A new task's flow refused, in the answers `daruda_flow_run` gives for the
/// same problems, so a caller reads them the same way.
fn flow_problem_answer(
    flow: Option<&daruda_store::tasks::TaskFlow>,
    problem: crate::workspace::task_flow::TaskFlowProblem,
) -> ControlError {
    use crate::workspace::task_flow::TaskFlowProblem as P;
    let name = flow.map(|flow| flow.name.clone()).unwrap_or_default();
    let input = || flow.and_then(|flow| flow.input.clone()).unwrap_or_default();
    let refused = |input: String, problem| ControlError::FlowInputsRefused {
        name: name.clone(),
        problems: vec![crate::control::result::FlowInputProblem { input, problem }],
    };
    use crate::control::result::FlowInputProblemKind as K;
    match problem {
        P::NotFound => ControlError::FlowNotFound { name },
        P::NeedsProfile => ControlError::FlowNeedsInteraction { name },
        P::UnknownProfile => ControlError::FlowUnknownProfile {
            profile: flow
                .and_then(|flow| flow.profile.clone())
                .unwrap_or_default(),
            name,
        },
        P::InputNotDeclared => refused(input(), K::NotDeclared),
        P::InputAmbiguous | P::OtherInputsRequired => ControlError::TaskStartFailed {
            detail: problem.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::result::LaneHandle;
    use daruda_store::project::WorkspaceUuid;

    /// Every failure after git registered the task's new worktree answers
    /// with that worktree's handle, keeping its cause's detail; the removed
    /// agent keeps its own code.
    #[test]
    fn a_failure_after_git_names_the_worktree_it_kept() {
        let workspace = WorkspaceUuid::new();
        let lane = LaneRef {
            project: 3,
            lane: 7,
        };
        let worktree = LaneHandle::new(workspace, lane);
        let kept = |cause: TaskStartError| control_start_error(cause.kept_in(lane), workspace);
        assert_eq!(
            kept(TaskStartError::AgentUnavailable),
            ControlError::AgentUnavailableWorktreeKept { worktree }
        );
        assert_eq!(
            kept(TaskStartError::PromptUndelivered),
            ControlError::TaskStartFailedWorktreeKept {
                worktree,
                detail: PROMPT_UNDELIVERED.into()
            }
        );
        assert_eq!(
            kept(TaskStartError::NotBacklog),
            ControlError::TaskStartFailedWorktreeKept {
                worktree,
                detail: ControlError::TaskNotBacklog.to_string()
            }
        );
        // Kept once: a failure already naming its worktree is not wrapped
        // again.
        assert_eq!(
            TaskStartError::NotBacklog.kept_in(lane).kept_in(LaneRef {
                project: 1,
                lane: 1
            }),
            TaskStartError::NotBacklog.kept_in(lane)
        );
    }
}
