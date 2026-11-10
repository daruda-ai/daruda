//! Starting a Backlog task, answered with its outcome.
//!
//! One start path for every caller. The Tasks UI drops the outcome — a
//! failure has already raised its toast here — while the control surface
//! waits on it, because a phone cannot see a desktop toast. A refusal is
//! answered before git is touched, so a caller hears it in the same turn.

use std::path::{Path, PathBuf};

use chrono::Utc;
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::tasks::{
    ExecutionRef, Lifecycle, StartedBy, TaskEvent, TaskExecutionSource, TaskRecord, TaskRunIn,
};
use gpui::{App, Context, Window};

use crate::runs::{GlobalTasks, RunsHost, TaskCommitError};
use crate::workspace::lane_ops::{CreateWorktreePlan, FinalizeError, LaneReveal};
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::{ChatAgent, ChatOpen, Workspace};
use daruda_chat::view::AgentSessionStatus;

mod error;

pub(crate) use error::{KeptWorktree, TaskStartError};

/// Where an accepted start handed the task's prompt over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskStarted {
    /// Identity captured by the start, even if the task has since run again.
    pub(crate) exec: ExecutionRef,
    pub(crate) worktree: PathBuf,
    pub(crate) on: StartedOn,
}

/// What a started run is driven by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartedOn {
    /// An Agent chat pane holds the prompt.
    Chat(PaneId),
    /// A flow run took the prompt as an input; it has no one pane.
    Flow,
}

impl TaskStarted {
    /// The Agent chat a started run talks in, if it has one.
    pub(crate) fn chat(&self) -> Option<PaneId> {
        match self.on {
            StartedOn::Chat(pane) => Some(pane),
            StartedOn::Flow => None,
        }
    }
}

/// The parts of a task's definition an approval card describes its start
/// by: where it runs (`run_in`, the new branch and the worktree it branches
/// from, the project), what it is called, what its agent is told, and the
/// flow it runs as. A start approved from a card carries the values the card
/// was written from and refuses to run any other definition, so an edit that
/// lands between the card and the start cannot move it somewhere unasked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShownTaskStart {
    project: daruda_store::project::ProjectUuid,
    title: String,
    prompt: String,
    run_in: TaskRunIn,
    branch: String,
    base: Option<PathBuf>,
    flow: Option<daruda_store::tasks::TaskFlow>,
}

impl ShownTaskStart {
    pub(crate) fn of(task: &TaskRecord) -> Self {
        let spec = &task.spec;
        Self {
            project: spec.project,
            title: spec.title.clone(),
            prompt: spec.prompt.clone(),
            run_in: spec.run_in.clone(),
            branch: spec.branch_name.clone(),
            base: spec.base_worktree_path.clone(),
            flow: spec.flow.clone(),
        }
    }

    /// Whether this start makes a new worktree, which is what spends an
    /// orchestrator's budget.
    pub(crate) fn creates_worktree(&self) -> bool {
        matches!(self.run_in, TaskRunIn::NewWorktree)
    }
}

pub(crate) type TaskStartOutcome = Result<TaskStarted, TaskStartError>;

pub(super) type Answer = smol::channel::Sender<TaskStartOutcome>;

impl Workspace {
    /// Start a Backlog task from the Tasks UI. The outcome is dropped: every
    /// failure the user can act on has raised its toast by then.
    pub(in crate::workspace) fn start_task(
        &mut self,
        task_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        drop(self.begin_task_start(task_id, StartedBy::Desktop, window, cx));
    }

    /// Start a Backlog task: record the start, open the lane (`git worktree
    /// add`, or a new tab in an existing lane for `TaskRunIn::ExistingLane`),
    /// then hand the pane the prompt.
    ///
    /// Answers on the returned channel once the pane holds the prompt. A
    /// refusal is on it before this returns — unless task commits are still
    /// landing, which the start first waits for, so a task saved a moment ago
    /// is found. A caller that stops listening cannot stall it: the channel
    /// holds one answer and is never awaited.
    pub(crate) fn begin_task_start(
        &mut self,
        task_id: &str,
        started_by: StartedBy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> smol::channel::Receiver<TaskStartOutcome> {
        let submitter = crate::runs::FlowSubmitter::of_task(started_by, false);
        self.begin_task_start_for(
            task_id,
            submitter,
            ChatAgent::Preferred(None),
            None,
            window,
            cx,
        )
    }

    /// [`Self::begin_task_start`] for `submitter`, whose solicitation a
    /// flow-backed task's run carries: a control start captured it before
    /// its card, and the run's questions follow it as the task's end does.
    /// A chat task opens `agent` (a gated start's card pinned it) in the
    /// mode `submitter` gets, so the two cannot be paired otherwise. A
    /// flow-backed task reads neither; its nodes name their own.
    /// `shown` is the definition a card approved: one that differs once the
    /// start reads the task is refused before anything is recorded.
    pub(crate) fn begin_task_start_for(
        &mut self,
        task_id: &str,
        submitter: crate::runs::FlowSubmitter,
        agent: ChatAgent,
        shown: Option<ShownTaskStart>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> smol::channel::Receiver<TaskStartOutcome> {
        let chat = ChatOpen {
            agent,
            mode: crate::workspace::mode_for(submitter.task_started_by()),
        };
        let (answer, rx) = smol::channel::bounded(1);
        let started = cx
            .global::<GlobalTasks>()
            .get(task_id)
            .is_some_and(|task| task.lifecycle != Lifecycle::Backlog);
        if started {
            self.finish_task_start(Err(TaskStartError::NotBacklog), None, &answer, cx);
            return rx;
        }
        if RunsHost::settled(cx) {
            let started = self.start_task_into(
                task_id,
                submitter,
                chat,
                shown.as_ref(),
                answer.clone(),
                window,
                cx,
            );
            if let Err(e) = started {
                self.finish_task_start(Err(e), None, &answer, cx);
            }
            return rx;
        }
        let mut barrier = RunsHost::barrier(cx);
        let id = task_id.to_owned();
        cx.spawn_in(window, async move |ws, cx| {
            loop {
                barrier.await;
                // The run's description is frozen from the task as the log
                // holds it when the start commits, so the start reads it only
                // with nothing landing: a proposal made during the wait would
                // otherwise commit before the start, unchecked and unread.
                let entered = ws.update_in(cx, |ws, window, cx| {
                    if !RunsHost::settled(cx) {
                        return Some(RunsHost::barrier(cx));
                    }
                    let started = ws.start_task_into(
                        &id,
                        submitter,
                        chat.clone(),
                        shown.as_ref(),
                        answer.clone(),
                        window,
                        cx,
                    );
                    if let Err(e) = started {
                        ws.finish_task_start(Err(e), None, &answer, cx);
                    }
                    None
                });
                match entered {
                    Ok(Some(next)) => barrier = next,
                    Ok(None) => break,
                    Err(_) => {
                        // SILENT-OK: the caller may have stopped listening.
                        let _ = answer.try_send(Err(TaskStartError::WindowClosed));
                        break;
                    }
                }
            }
        })
        .detach();
        rx
    }

    /// The one place a start's outcome is delivered: the failure recorded
    /// for a run that was requested, the toast, then the answer.
    pub(super) fn finish_task_start(
        &mut self,
        outcome: TaskStartOutcome,
        exec: Option<&ExecutionRef>,
        answer: &Answer,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = &outcome {
            if let Some(exec) = exec {
                fail_start(exec, e, cx);
            }
            if let Some(report) = e.report() {
                self.report_error(report, cx);
            }
        }
        // SILENT-OK: a full or closed channel means the caller stopped
        // listening; the start itself is done either way.
        let _ = answer.try_send(outcome);
    }

    /// Check the task can start, record the request, then continue once the
    /// log holds it. `Err` is a refusal taken before anything was recorded or
    /// spawned, leaving the task in Backlog.
    #[allow(clippy::too_many_arguments)] // Each is one caller-held fact the
    // two entry branches (settled now, or after the barrier) hand over as is.
    fn start_task_into(
        &mut self,
        task_id: &str,
        submitter: crate::runs::FlowSubmitter,
        chat: ChatOpen,
        shown: Option<&ShownTaskStart>,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), TaskStartError> {
        let started_by = submitter.task_started_by();
        let task = cx
            .global::<GlobalTasks>()
            .get(task_id)
            .cloned()
            .ok_or(TaskStartError::NotFound)?;
        if task.lifecycle != Lifecycle::Backlog {
            return Err(TaskStartError::NotBacklog);
        }
        // The definition read here, after any commit still landing, is the
        // one the start runs; the lock, git and the log come after.
        if shown.is_some_and(|shown| *shown != ShownTaskStart::of(&task)) {
            return Err(TaskStartError::ChangedSinceShown);
        }
        let reveal = reveal_for(started_by);
        let route = match &task.spec.flow {
            Some(flow) => StartRoute::Flow(Box::new(
                self.prepare_task_flow_start(&task, flow, reveal, submitter, cx)?,
            )),
            None => {
                // Resolved now, so a desktop start keeps the agent it was
                // asked under; a pinned one is checked again as its pane opens.
                let resolved = self
                    .resolve_chat_agent(&chat.agent)
                    .map_err(|_| TaskStartError::AgentUnavailable)?;
                let chat = match chat.agent {
                    ChatAgent::Pinned(_) => chat,
                    ChatAgent::Preferred(_) => ChatOpen {
                        agent: ChatAgent::Preferred(Some(resolved.clone())),
                        mode: chat.mode,
                    },
                };
                match &task.spec.run_in {
                    // An existing lane needs no git repo, lock or checkout.
                    TaskRunIn::ExistingLane { path } => StartRoute::Existing {
                        start: self.prepare_existing_lane_start(path, &resolved, reveal)?,
                        chat,
                    },
                    TaskRunIn::NewWorktree => StartRoute::New {
                        start: self.prepare_worktree_start(&task, reveal)?,
                        chat,
                    },
                }
            }
        };
        let exec = ExecutionRef {
            task_id: task.id().to_owned(),
            execution_id: uuid::Uuid::new_v4().to_string(),
        };
        let source = match &route {
            StartRoute::Flow(start) => start.source(),
            StartRoute::Existing { .. } | StartRoute::New { .. } => TaskExecutionSource::AgentChat,
        };
        let requested = RunsHost::task(
            crate::runs::TaskIntent::Start {
                exec: exec.clone(),
                source,
                started_by,
            },
            cx,
        );
        cx.spawn_in(window, async move |ws, cx| {
            let recorded = requested.await;
            let continued = ws.update_in(cx, |ws, window, cx| match recorded {
                Ok(_) => ws.continue_task_start(route, exec.clone(), answer.clone(), window, cx),
                Err(e) => {
                    if let Some(repo_root) = route.repo_lock() {
                        ws.release_repo_lock(repo_root);
                    }
                    let refused = match e {
                        TaskCommitError::Rejected(_) => TaskStartError::NotBacklog,
                        other => TaskStartError::NotRecorded {
                            detail: other.to_string(),
                        },
                    };
                    ws.finish_task_start(Err(refused), None, &answer, cx);
                }
            });
            if continued.is_err() {
                abandon_start(&exec, TaskStartError::WindowClosed, &answer, cx);
            }
        })
        .detach();
        Ok(())
    }

    /// Everything a new worktree needs, checked before the start is
    /// recorded; holds the repository's lock from here on.
    pub(super) fn prepare_worktree_start(
        &mut self,
        task: &TaskRecord,
        reveal: LaneReveal,
    ) -> Result<WorktreeStart, TaskStartError> {
        let project_id = self
            .project_by_uuid(task.spec.project)
            .map(|p| p.id)
            .ok_or(TaskStartError::ProjectNotOpen)?;
        let repo_root = self
            .git_repo_root_for_project(project_id)
            .ok_or(TaskStartError::NoGitRepo)?;
        if !self.acquire_repo_lock(&repo_root) {
            return Err(TaskStartError::RepoBusy);
        }
        let spec = &task.spec;
        let new_path =
            crate::workspace::lane_ops::lane_checkout_path(&repo_root, &spec.branch_name);
        let base_ref = spec
            .base_worktree_path
            .as_deref()
            .and_then(|p| self.branch_for_worktree_path(p));
        let plan = CreateWorktreePlan {
            branch: spec.branch_name.clone(),
            new_path,
            repo_root,
            base_ref: self.resolve_lane_base_ref_for(project_id, base_ref),
            description: Some(crate::surface::strings::task::lane_description(&spec.title)),
            // Task-driven lanes have no create-form host picker — they stay
            // at `Lane::git`'s default (unanswered/Local).
            session_host: None,
        };
        Ok(WorktreeStart {
            plan,
            project_id,
            reveal,
        })
    }

    /// The start is recorded: create the worktree or open the pane, then
    /// dispatch. Every failure from here is recorded against `exec`.
    fn continue_task_start(
        &mut self,
        route: StartRoute,
        exec: ExecutionRef,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match route {
            StartRoute::Existing { start, chat } => {
                let started = self.start_task_in_existing_lane(&exec, start, chat, window, cx);
                self.answer_when_dispatched(started, exec, answer, cx);
            }
            StartRoute::New { start, chat } => {
                self.create_task_worktree(start, chat, exec, answer, window, cx)
            }
            StartRoute::Flow(start) => {
                self.continue_task_flow_start(*start, exec, answer, window, cx)
            }
        }
    }

    fn create_task_worktree(
        &mut self,
        start: WorktreeStart,
        chat: ChatOpen,
        exec: ExecutionRef,
        answer: Answer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let WorktreeStart {
            plan,
            project_id,
            reveal,
        } = start;
        let me = cx.weak_entity();
        let repo = crate::git_host::GitHost::global(cx).open(&plan.repo_root);
        window
            .spawn(cx, async move |async_cx| {
                let result: Result<(), String> = async_cx
                    .background_executor()
                    .spawn({
                        let plan = plan.clone();
                        async move {
                            repo.add_lane(
                                &plan.new_path,
                                Some(&plan.branch),
                                plan.base_ref.as_deref(),
                            )
                            .map_err(|e| e.to_string())
                        }
                    })
                    .await;
                let gone = window_closed_after(&result, &plan.new_path);

                let update_result = async_cx.update(|window, app_cx| {
                    let Some(workspace) = me.upgrade() else {
                        return false;
                    };
                    workspace.update(app_cx, |ws, cx| {
                        // Lock is released *before* finalize_chat_lane runs,
                        // so a retry against the same repo isn't blocked.
                        // The worktree is on disk either way: a project
                        // closed meanwhile leaves it unregistered, named by
                        // its path; a removed agent registers it with
                        // nothing open.
                        ws.release_repo_lock(&plan.repo_root);
                        let started = result
                            .map_err(|detail| TaskStartError::GitAddFailed { detail })
                            .and_then(|()| {
                                ws.finalize_chat_lane(
                                    plan.clone(),
                                    project_id,
                                    chat.clone(),
                                    reveal,
                                    window,
                                    cx,
                                )
                                .map_err(|e| match e {
                                    FinalizeError::ProjectClosed => TaskStartError::ProjectNotOpen
                                        .left_unregistered(plan.new_path.clone()),
                                    FinalizeError::AgentUnavailable { lane } => {
                                        TaskStartError::AgentUnavailable.kept_in(lane)
                                    }
                                })
                            })
                            .and_then(|pane| {
                                // From here on a failure leaves the
                                // worktree registered with the pane.
                                let lane = ws.lane_ref_at(&plan.new_path);
                                let kept = move |e: TaskStartError| match lane {
                                    Some(lane) => e.kept_in(lane),
                                    None => e,
                                };
                                let dispatched = ws
                                    .dispatch_task_to_chat(
                                        &exec,
                                        &plan.new_path,
                                        &plan.branch,
                                        pane,
                                        window,
                                        cx,
                                    )
                                    .map_err(kept)?;
                                let dispatched =
                                    cx.spawn(async move |_, _| dispatched.await.map_err(kept));
                                Ok((
                                    TaskStarted {
                                        exec: exec.clone(),
                                        worktree: plan.new_path.clone(),
                                        on: StartedOn::Chat(pane),
                                    },
                                    dispatched,
                                ))
                            });
                        ws.answer_when_dispatched(started, exec.clone(), answer.clone(), cx);
                    });
                    true
                });
                if !matches!(update_result, Ok(true)) {
                    LogWriter::log(
                        ErrorReport::new("Task workflow completion could not reach workspace")
                            .severity(ErrorSeverity::Warning)
                            .at(file!(), line!())
                            .with_context("error", format!("{update_result:?}"))
                            .dedup("task_workflow.completion.update_failed")
                            .build(),
                    );
                    abandon_start(&exec, gone, &answer, async_cx);
                }
            })
            .detach();
    }

    /// Answer after the preparation and delivery result is projected. Chat
    /// queueing leaves the task Dispatching until its PromptSent arrives.
    pub(super) fn answer_when_dispatched(
        &mut self,
        started: Result<(TaskStarted, gpui::Task<Result<(), TaskStartError>>), TaskStartError>,
        exec: ExecutionRef,
        answer: Answer,
        cx: &mut Context<Self>,
    ) {
        let (started, dispatched) = match started {
            Ok(started) => started,
            Err(e) => {
                self.finish_task_start(Err(e), Some(&exec), &answer, cx);
                return;
            }
        };
        cx.spawn(async move |ws, cx| {
            let outcome = dispatched.await.map(|()| started);
            let finished = ws.update(cx, |ws, cx| {
                ws.finish_task_start(outcome.clone(), None, &answer, cx)
            });
            if finished.is_err() {
                // SILENT-OK: the caller may have stopped listening.
                let _ = answer.try_send(outcome);
            }
        })
        .detach();
    }

    /// Bind chat `pane_id` to the run, record the pane's identity, and hand
    /// it the prompt as an ACP turn. Preparation is durable before delivery;
    /// only the turn's tagged PromptSent records Dispatched.
    ///
    /// Takes `pane_id` rather than relying on `focused_pane_id`: a start
    /// asked for elsewhere opens its pane behind the one the window shows.
    pub(super) fn dispatch_task_to_chat(
        &mut self,
        exec: &ExecutionRef,
        worktree_path: &Path,
        branch: &str,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<gpui::Task<Result<(), TaskStartError>>, TaskStartError> {
        // A cancel that landed while the lane was made ends the start here.
        let task = cx
            .global::<GlobalTasks>()
            .get(&exec.task_id)
            .filter(|task| matches!(&task.lifecycle, Lifecycle::Preparing { exec: current, .. } if current == exec))
            .cloned()
            .ok_or(TaskStartError::NotBacklog)?;
        let identity = self
            .bind_task_chat_execution(exec, pane_id, cx)
            .ok_or(TaskStartError::PaneUnavailable)?;
        let prepared = RunsHost::propose(
            TaskEvent::LanePrepared {
                terminal_id: None,
                exec: exec.clone(),
                worktree: worktree_path.to_path_buf(),
                agent_id: identity.agent_id,
                account_id: identity.account_id,
                cwd: identity.cwd,
                at: Utc::now(),
            },
            cx,
        );
        let exec = exec.clone();
        let branch = branch.to_owned();
        Ok(cx.spawn_in(window, async move |ws, cx| {
            prepared
                .await
                .map_err(|error| TaskStartError::NotRecorded {
                    detail: error.to_string(),
                })?;
            let result = ws
                .update_in(cx, |ws, window, cx| {
                    ws.deliver_prepared_task(&exec, &task, &branch, pane_id, window, cx)
                })
                .unwrap_or(Err(TaskStartError::WindowClosed));
            if let Err(error) = &result {
                gpui::AsyncApp::update(cx, |cx| fail_start(&exec, error, cx));
            }
            result
        }))
    }

    fn deliver_prepared_task(
        &mut self,
        exec: &ExecutionRef,
        task: &TaskRecord,
        branch: &str,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), TaskStartError> {
        if RunsHost::pane_execution(self.run_pane(pane_id), cx) != Some(exec) {
            return Err(TaskStartError::PaneUnavailable);
        }
        if !cx.global::<GlobalTasks>().get(&exec.task_id).is_some_and(|task| {
            matches!(&task.lifecycle, Lifecycle::Dispatching { exec: current, .. } if current == exec)
        }) {
            return Err(TaskStartError::NotBacklog);
        }
        let rendered = daruda_store::tasks::render_task_prompt(task, branch);
        let failed = self
            .agent_chat_view(pane_id)
            .is_some_and(|view| matches!(view.read(cx).status(), AgentSessionStatus::Error { .. }));
        let input = crate::workspace::main_area::pane_input_ops::PaneTextInput {
            body: rendered,
            intent: crate::workspace::main_area::pane_input_ops::PaneTextIntent::Command {
                submit: true,
            },
            task: Some(exec.clone()),
        };
        if failed || !self.deliver_text_to_pane(pane_id, input, window, cx) {
            RunsHost::delivery_failed(
                exec.clone(),
                crate::runs::DeliveryFailure::Dropped(
                    daruda_store::tasks::machine::UndeliveredReason::StartFailed,
                ),
                cx,
            );
            return Err(TaskStartError::PromptUndelivered);
        }
        cx.notify();
        Ok(())
    }
}

/// A new worktree's start, checked and holding the repository's lock.
pub(super) struct WorktreeStart {
    pub(super) plan: CreateWorktreePlan,
    pub(super) project_id: daruda_store::project::ProjectId,
    pub(super) reveal: LaneReveal,
}

/// Only a start made at this window brings its lane forward. One the phone
/// or the orchestrator asked for runs behind the view the person is using —
/// often the orchestrator's own tab, which a lane switch would take down.
fn reveal_for(started_by: StartedBy) -> LaneReveal {
    match started_by {
        StartedBy::Desktop => LaneReveal::Foreground,
        StartedBy::Phone | StartedBy::Orchestrator => LaneReveal::Background,
    }
}

/// Where a recorded start continues.
enum StartRoute {
    Existing {
        start: super::task_existing_lane_ops::ExistingLaneStart,
        chat: ChatOpen,
    },
    New {
        start: WorktreeStart,
        chat: ChatOpen,
    },
    /// Boxed because a flow start carries a whole worktree start plus its
    /// inputs, about twice the other routes once Windows paths widen it.
    Flow(Box<super::task_flow_start::TaskFlowStart>),
}

impl StartRoute {
    /// The repository whose lock the start holds until its worktree exists.
    fn repo_lock(&self) -> Option<&Path> {
        match self {
            Self::New { start, .. } => Some(&start.plan.repo_root),
            Self::Flow(start) => start.repo_lock(),
            Self::Existing { .. } => None,
        }
    }
}

/// What the pane a run uses is, as `LanePrepared` records it.
pub(super) struct PaneIdentity {
    pub(super) agent_id: String,
    pub(super) account_id: Option<daruda_store::accounts::AccountId>,
    pub(super) cwd: PathBuf,
}

/// Record why a requested start did not dispatch.
pub(super) fn fail_start(exec: &ExecutionRef, e: &TaskStartError, cx: &mut App) {
    let Some(cause) = e.failure() else { return };
    RunsHost::delivery_failed(exec.clone(), crate::runs::DeliveryFailure::Start(cause), cx);
}

/// The window closed once `git worktree add` answered `added`: a checkout
/// git made is left on disk with no lane, so the failure names its `path`.
pub(super) fn window_closed_after(added: &Result<(), String>, path: &Path) -> TaskStartError {
    match added {
        Ok(()) => TaskStartError::WindowClosed.left_unregistered(path.to_path_buf()),
        Err(_) => TaskStartError::WindowClosed,
    }
}

/// The window closed under a recorded start: record it, then answer with
/// `closed` — `WindowClosed`, wrapped with the worktree the start left on
/// disk if it made one.
pub(super) fn abandon_start(
    exec: &ExecutionRef,
    closed: TaskStartError,
    answer: &Answer,
    cx: &gpui::AsyncApp,
) {
    cx.update(|cx| {
        fail_start(exec, &closed, cx);
        // SILENT-OK: the caller may have stopped listening.
        let _ = answer.try_send(Err(closed));
    });
}

#[cfg(test)]
#[path = "../task_start_tests.rs"]
mod tests;
