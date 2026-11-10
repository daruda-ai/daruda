//! What an approval card pins of a task's start, read the way the start
//! reads it, so an edit landing between the card and the start is refused.

use std::path::PathBuf;

use daruda_store::tasks::{TaskRecord, TaskRunIn};

use crate::workspace::Workspace;

/// The parts of a task's definition an approval card describes its start
/// by: where it runs (`run_in`, the new branch and the worktree it branches
/// from, the project), what it is called, what its agent is told, and the
/// flow it runs as. A start approved from a card carries the values the card
/// was written from and refuses to run any other definition, so an edit that
/// lands between the card and the start cannot move it somewhere unasked.
///
/// It also holds what the definition resolved to when the card was written,
/// read the way the start reads it: the branch whose code the run starts
/// from (`checkout`) and the flow file its name found (`flow_file`). A card
/// names a worktree and a flow's modes; the worktree checking out another
/// branch, or the flow file changing or being shadowed, while the card waits
/// would run code or modes it never showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShownTaskStart {
    project: daruda_store::project::ProjectUuid,
    title: String,
    prompt: String,
    run_in: TaskRunIn,
    branch: String,
    base: Option<PathBuf>,
    flow: Option<daruda_store::tasks::TaskFlow>,
    /// See [`Workspace::task_start_checkout`].
    checkout: Option<String>,
    flow_file: Option<crate::workspace::task_flow::FlowFileSeen>,
}

impl ShownTaskStart {
    /// Whether this start makes a new worktree, which is what spends an
    /// orchestrator's budget.
    pub(crate) fn creates_worktree(&self) -> bool {
        matches!(self.run_in, TaskRunIn::NewWorktree)
    }
}

impl Workspace {
    /// `task`'s start as a card describes it, its flow file read now.
    pub(crate) fn shown_task_start(&self, task: &TaskRecord) -> ShownTaskStart {
        let spec = &task.spec;
        let flow_file = spec.flow.as_ref().and_then(|flow| {
            self.task_flow_file(
                spec.project,
                &spec.run_in,
                spec.base_worktree_path.as_deref(),
                flow,
            )
        });
        self.shown_task_start_with(task, flow_file)
    }

    /// [`Self::shown_task_start`] with the flow file a card's flow summary
    /// was read from, so the card and what it pins come from one read.
    pub(in crate::workspace) fn shown_task_start_with(
        &self,
        task: &TaskRecord,
        flow_file: Option<crate::workspace::task_flow::FlowFileSeen>,
    ) -> ShownTaskStart {
        let spec = &task.spec;
        ShownTaskStart {
            project: spec.project,
            title: spec.title.clone(),
            prompt: spec.prompt.clone(),
            run_in: spec.run_in.clone(),
            branch: spec.branch_name.clone(),
            base: spec.base_worktree_path.clone(),
            flow: spec.flow.clone(),
            checkout: self.task_start_checkout(spec),
            flow_file,
        }
    }

    /// The branch whose code a start of `spec` runs on: for a new worktree,
    /// the ref its branch is cut from ([`Self::task_base_ref`]); for an
    /// existing lane, the branch it has checked out. `None` when there is
    /// none to name (a detached or plain lane, or a project not open).
    pub(in crate::workspace) fn task_start_checkout(
        &self,
        spec: &daruda_store::tasks::TaskSpec,
    ) -> Option<String> {
        match &spec.run_in {
            TaskRunIn::NewWorktree => {
                let project_id = self.project_by_uuid(spec.project)?.id;
                self.task_base_ref(project_id, spec)
            }
            TaskRunIn::ExistingLane { path } => self
                .lane_ref_at(path)
                .and_then(|lane| self.lane_for(lane))
                .and_then(super::super::task_existing_lane_ops::lane_branch)
                .map(str::to_owned),
        }
    }

    /// The ref a new worktree for `spec` branches from: the branch its base
    /// worktree has checked out, else the project's base branch. The start
    /// and its card both resolve it here.
    pub(super) fn task_base_ref(
        &self,
        project_id: daruda_store::project::ProjectId,
        spec: &daruda_store::tasks::TaskSpec,
    ) -> Option<String> {
        let base = spec
            .base_worktree_path
            .as_deref()
            .and_then(|p| self.branch_for_worktree_path(p));
        self.resolve_lane_base_ref_for(project_id, base)
    }
}
