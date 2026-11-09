//! Commands that wait on the user's approval: the guards before the card,
//! the work once approved, and the worktree creation it runs. Task starts
//! and opens live in `tasks`, which other callers reach without a card.

use futures::{FutureExt as _, future::LocalBoxFuture};
use gpui::App;

use crate::control::approval::ApprovalOutcome;
use crate::control::result::{ControlError, ControlOutcome, ControlResult, LaneHandle};
use crate::control::spec::{GatedCommand, NewTask};
use crate::remote_channel::bridge::PaneRef;
use crate::runs::GlobalTasks;
use crate::surface::strings as s;

use super::{Dispatch, Pending, in_lane_window, in_pane_window, in_project_window, in_task_window};

mod disclosure;
mod flow_resume;
mod tasks;

pub(crate) use disclosure::quoted_title;
pub(crate) use tasks::{begin_task_start, open_task, task_flow_summary};
use tasks::{start_task, task_title};

/// A command as its card asked about it. A resume's run is resolved once,
/// before the card, so a handle such as `TaskLatest` cannot name another
/// execution by the time the user answers; no variant carries the handle.
/// The same holds for the agent a chat opens with: the card may wait for
/// minutes while the window's last agent changes, so it is a concrete id.
enum Asked {
    LaneCreate {
        workspace: daruda_store::project::WorkspaceUuid,
        project: daruda_store::project::ProjectId,
        name: String,
        base_ref: Option<String>,
        agent: String,
        prompt: Option<String>,
    },
    ChatNew {
        lane: LaneHandle,
        agent: String,
    },
    TaskCreate {
        workspace: daruda_store::project::WorkspaceUuid,
        project: daruda_store::project::ProjectId,
        task: NewTask,
    },
    /// `agent` is unused by a flow-backed task, whose nodes name their own.
    TaskStart {
        task: String,
        agent: String,
    },
    TaskOpen {
        task: String,
    },
    FlowResume(flow_resume::ResumeTarget),
}

impl Asked {
    /// Resolve a resume's run and the agent a chat opens with, in the window
    /// that will open it; every other command is asked about as sent. Not
    /// resumable, gone or held by another run: no answer changes that, so
    /// the card is not shown.
    fn resolve(cmd: GatedCommand, cx: &mut App) -> Result<Self, ControlError> {
        Ok(match cmd {
            GatedCommand::LaneCreate {
                workspace,
                project,
                name,
                base_ref,
                agent,
                prompt,
            } => Self::LaneCreate {
                workspace,
                project,
                name,
                base_ref,
                agent: in_project_window(workspace, project, cx, |ws, _, _| {
                    Ok(ws.open_agent_id(agent.as_deref()))
                })?,
                prompt,
            },
            GatedCommand::ChatNew { lane, agent } => Self::ChatNew {
                lane,
                agent: in_lane_window(lane, cx, |ws, _, _| Ok(ws.open_agent_id(agent.as_deref())))?,
            },
            GatedCommand::TaskCreate {
                workspace,
                project,
                task,
            } => Self::TaskCreate {
                workspace,
                project,
                task,
            },
            GatedCommand::TaskStart { task } => Self::TaskStart {
                agent: in_task_window(&task, cx, |ws, _, _| Ok(ws.open_agent_id(None)))?,
                task,
            },
            GatedCommand::TaskOpen { task } => Self::TaskOpen { task },
            GatedCommand::FlowResume { run } => Self::FlowResume(flow_resume::target(&run, cx)?),
        })
    }
}

/// Run a command that has to wait: guards, then the user's approval, then the
/// work.
///
/// The order matters. A budget the agent has spent and a pane it may not
/// address are refusals the user has no say in, so they are answered before a
/// card is ever put on their phone.
pub(crate) fn run_gated(cmd: GatedCommand, origin: super::CommandOrigin, cx: &mut App) -> Dispatch {
    run_gated_with(
        cmd,
        origin,
        |cmd, mode, caller, cx| perform_gated(cmd, mode, caller, cx).boxed_local(),
        cx,
    )
}

fn run_gated_with(
    cmd: GatedCommand,
    origin: super::CommandOrigin,
    perform: impl for<'a> FnOnce(
        Asked,
        daruda_config::ModeChoice,
        super::Caller,
        &'a mut gpui::AsyncApp,
    ) -> LocalBoxFuture<'a, ControlOutcome>
    + 'static,
    cx: &mut App,
) -> Dispatch {
    let asked = match Asked::resolve(cmd, cx) {
        Ok(asked) => asked,
        Err(refusal) => return Dispatch::Ready(Err(refusal)),
    };
    if let Err(refusal) = guard_gated(&asked, cx) {
        return Dispatch::Ready(Err(refusal));
    }
    // The mode every chat this command opens takes, named on its card and
    // handed to the work as it is, so the two cannot disagree.
    let mode = crate::workspace::mode_for(origin.started_by());
    let summary = match approval_summary(&asked, &mode, cx) {
        Ok(summary) => summary,
        Err(refusal) => return Dispatch::Ready(Err(refusal)),
    };
    let caller = origin.capture(cx);
    let delivery = caller.delivery(crate::app_presence::is_away(cx));
    let (approval, answer) = crate::control::approval::request(summary, delivery, cx);
    let (tx, rx) = smol::channel::bounded(1);
    cx.spawn(async move |cx| {
        let outcome: Option<ControlOutcome> = match answer.recv().await {
            // Approval may have waited while another creation spent a slot.
            // Reserve before the first await so overlapping approvals cannot
            // promise the same remaining worktree.
            Ok(ApprovalOutcome::Approved) => {
                Some(match cx.update(|cx| reserve_gated(&asked, cx)) {
                    Ok(permit) => {
                        let outcome = perform(asked, mode, caller, cx).await;
                        if let Some(permit) = permit {
                            cx.update(|cx| permit.finish(left_a_worktree(&outcome), cx));
                        }
                        outcome
                    }
                    Err(refusal) => Err(refusal),
                })
            }
            // Not a refusal: the user never said no, and the caller has freed
            // the id it would be answered on. Nothing ran and nothing is owed,
            // which is what `None` says and no error variant could.
            Ok(ApprovalOutcome::Withdrawn) => None,
            Ok(ApprovalOutcome::Refused) => Some(Err(ControlError::ApprovalRefused)),
            Ok(ApprovalOutcome::Undeliverable) => Some(Err(ControlError::ApprovalUnavailable)),
            Ok(ApprovalOutcome::TooManyPending) => Some(Err(ControlError::ApprovalsPending)),
            // A dropped channel means the app is going away, which the caller
            // cannot act on either — report it as the unanswered card it is.
            Ok(ApprovalOutcome::TimedOut) | Err(_) => Some(Err(ControlError::ApprovalTimedOut)),
        };
        let _ = tx.send(outcome).await;
    })
    .detach();
    Dispatch::Deferred {
        outcome: rx,
        pending: Pending::Approval(approval),
    }
}

/// Validate again and claim a slot only for commands that create a worktree.
/// A resume checks its run again as it resumes.
fn reserve_gated(
    asked: &Asked,
    cx: &mut App,
) -> Result<Option<crate::control::guards::CreationPermit>, ControlError> {
    if let Asked::FlowResume(_) = asked {
        return Ok(None);
    }
    guard_gated(asked, cx)?;
    let creates_lane = match asked {
        Asked::LaneCreate { .. } => true,
        Asked::TaskStart { task, .. } => cx
            .global::<GlobalTasks>()
            .get(task)
            .is_some_and(task_creates_lane),
        _ => false,
    };
    creates_lane
        .then(|| crate::control::guards::reserve_agent_lane(cx))
        .transpose()
}

/// The refusals that precede the card.
fn guard_gated(asked: &Asked, cx: &mut App) -> Result<(), ControlError> {
    match asked {
        Asked::LaneCreate { name, .. } => {
            // Before the card, not with the plan that quotes it: a name git
            // will reject cannot become a worktree however the user answers,
            // so asking them would cost a tap and then blame them for a
            // mistake they did not make. Pure — the same predicate
            // `control_lane_plan` sanitizes with, asked earlier.
            if daruda_core::git::sanitize_branch_name(name).is_none() {
                return Err(ControlError::LaneNameInvalid);
            }
            crate::control::guards::guard_agent_budget(cx)
        }
        // Opening a chat costs no budget: it is one pane in a worktree the
        // user already has, and the gate is the whole check.
        Asked::ChatNew { lane, .. } => {
            in_lane_window(*lane, cx, |_ws, _window, _cx| Ok(())).map(|_| ())
        }
        // The same checks the create runs once approved, so a branch the
        // Task editor would refuse is answered before the card asks about it.
        Asked::TaskCreate {
            workspace,
            project,
            task,
        } => in_project_window(*workspace, *project, cx, |ws, _window, _cx| {
            ws.control_task_spec(*project, task.clone()).map(|_| ())
        }),
        // Only a start that creates a worktree spends the budget, as
        // `daruda_worktree_create` does; an existing one is already there.
        Asked::TaskStart { task, .. } => {
            let tasks = cx.global::<GlobalTasks>();
            let found = tasks.get(task).ok_or(ControlError::TaskNotFound)?;
            if found.lifecycle != daruda_store::tasks::Lifecycle::Backlog {
                return Err(ControlError::TaskNotBacklog);
            }
            let creates_lane = task_creates_lane(found);
            in_task_window(task, cx, |_ws, _window, _cx| Ok(()))?;
            if creates_lane {
                crate::control::guards::guard_agent_budget(cx)?;
            }
            Ok(())
        }
        Asked::TaskOpen { task } => in_task_window(task, cx, |_ws, _window, _cx| Ok(())),
        // Resolved before the card: the run's state is read then, and once
        // more by the resume itself.
        Asked::FlowResume(_) => Ok(()),
    }
}

/// Whether a creation spent its slot: a refusal that kept its worktree
/// made one as surely as a success did.
fn left_a_worktree(outcome: &ControlOutcome) -> bool {
    matches!(
        outcome,
        Ok(_) | Err(ControlError::AgentUnavailableWorktreeKept { .. })
    )
}

/// `task`'s prompt as its start card quotes it; see `prompt_phrase`.
fn task_prompt(task: &str, cx: &App) -> Option<String> {
    cx.global::<GlobalTasks>()
        .get(task)
        .and_then(|t| disclosure::prompt_phrase(&t.spec.prompt))
}

fn task_creates_lane(task: &daruda_store::tasks::TaskRecord) -> bool {
    matches!(
        task.spec.run_in,
        daruda_store::tasks::TaskRunIn::NewWorktree
    )
}

/// One line naming what the user is being asked to allow, built here because
/// `GatedCommand` carries no rendered sentences. A target that cannot be
/// named is refused rather than asked about. A chat's card names its pinned
/// agent and the mode `mode` resolves to; others' titles and prompts pass
/// through `agent_text` first.
fn approval_summary(
    asked: &Asked,
    mode: &daruda_config::ModeChoice,
    cx: &mut App,
) -> Result<String, ControlError> {
    use disclosure::{agent_phrase, prompt_phrase};
    Ok(match asked {
        Asked::LaneCreate {
            workspace,
            project,
            name,
            agent,
            prompt,
            ..
        } => {
            let agent = in_project_window(*workspace, *project, cx, |ws, _, _| {
                Ok(agent_phrase(&ws.control_chat_disclosure(agent, mode)))
            })?;
            match prompt.as_deref().and_then(prompt_phrase) {
                Some(prompt) => s::control::approval_lane_create_prompt(name, agent, prompt),
                None => s::control::approval_lane_create(name, agent),
            }
        }
        Asked::ChatNew { lane, agent } => {
            let (path, agent) = in_lane_window(*lane, cx, |ws, _window, _cx| {
                let path = s::control::lane_path(
                    ws.control_project_name(lane.project),
                    ws.control_lane_name(lane.lane_ref()),
                );
                Ok((path, agent_phrase(&ws.control_chat_disclosure(agent, mode))))
            })?;
            s::control::approval_chat_new(&path, agent)
        }
        Asked::TaskCreate {
            workspace,
            project,
            task,
        } => {
            let (name, location, flow) =
                in_project_window(*workspace, *project, cx, |ws, _, _| {
                    // The flow is named as Start will find it; one that will not
                    // resolve is refused by the create itself, after approval.
                    let flow = task.flow.as_ref().map(|flow| {
                        ws.control_new_task_flow_approval(
                            *project,
                            task.worktree.map(|lane| lane.lane_ref()),
                            flow,
                        )
                    });
                    let location = match task.worktree {
                        Some(lane) => s::control::approval_task_in_worktree(
                            ws.control_lane_name(lane.lane_ref()),
                        ),
                        None => match task
                            .branch
                            .as_deref()
                            .map(str::trim)
                            .filter(|b| !b.is_empty())
                        {
                            Some(branch) => s::control::approval_task_in_new_branch(branch),
                            None => s::control::approval_task_in_new_worktree(),
                        },
                    };
                    Ok((ws.control_project_name(*project), location, flow))
                })?;
            let surface = flow.unwrap_or_else(s::control::approval_task_as_chat);
            s::control::approval_task_create(quoted_title(&task.title), &name, &location, &surface)
        }
        // A flow given the prompt hands it to its agents, so its card quotes
        // it as a chat's does.
        Asked::TaskStart { task, agent } => match task_flow_summary(task, cx) {
            Some(flow) => match flow.takes_prompt.then(|| task_prompt(task, cx)).flatten() {
                Some(prompt) => s::control::approval_task_start_flow_prompt(
                    task_title(task, cx),
                    &flow.summary,
                    prompt,
                ),
                None => s::control::approval_task_start_flow(task_title(task, cx), &flow.summary),
            },
            None => {
                let prompt = task_prompt(task, cx);
                let agent = in_task_window(task, cx, |ws, _, _| {
                    Ok(agent_phrase(&ws.control_chat_disclosure(agent, mode)))
                })?;
                match prompt {
                    Some(prompt) => {
                        s::control::approval_task_start(task_title(task, cx), agent, prompt)
                    }
                    None => s::control::approval_task_start_no_prompt(task_title(task, cx), agent),
                }
            }
        },
        // A chat already open keeps its own mode; only a new pane takes `mode`.
        // Which one this is is read now: a pane opened or closed before the
        // answer can make the card's mode claim stale. The open then follows
        // the pane as it finds it, with the card's `mode` for a new one.
        Asked::TaskOpen { task } => {
            let agent = in_task_window(task, cx, |ws, _, cx| {
                Ok(ws
                    .task_chat_reopen_agent(task, cx)
                    .map(|agent| agent_phrase(&ws.control_chat_disclosure(&agent, mode))))
            })?;
            match agent {
                Some(agent) => s::control::approval_task_open_new(task_title(task, cx), agent),
                None => s::control::approval_task_open(task_title(task, cx)),
            }
        }
        Asked::FlowResume(target) => flow_resume::summary(target, cx)?,
    })
}

/// Do the thing, now that the user has said yes. A chat it opens takes
/// `mode`, the one its card named.
async fn perform_gated(
    asked: Asked,
    mode: daruda_config::ModeChoice,
    caller: super::Caller,
    cx: &mut gpui::AsyncApp,
) -> ControlOutcome {
    let created = match asked {
        Asked::FlowResume(target) => {
            return cx.update(|cx| flow_resume::resume(target, caller, cx));
        }
        Asked::TaskCreate {
            workspace,
            project,
            task,
        } => {
            // Answered once the task is in the log, so a start or a listing
            // the caller sends next finds it.
            let created = cx.update(|cx| {
                in_project_window(workspace, project, cx, |ws, _window, cx| {
                    ws.control_task_create(project, task.clone(), cx)
                })
            })?;
            return created
                .await
                .map(|task| ControlResult::TaskCreated { task });
        }
        // The start derives the same mode from `caller`, the origin the card
        // read it from.
        Asked::TaskStart { task, agent } => return start_task(task, agent, caller, cx).await,
        Asked::TaskOpen { task } => {
            return cx.update(|cx| tasks::open_task_in(task, mode, caller, cx));
        }
        Asked::ChatNew { lane, agent } => cx
            .update(|cx| {
                in_lane_window(lane, cx, |ws, window, cx| {
                    ws.control_chat_new(
                        lane.lane_ref(),
                        pinned_chat(agent.clone(), mode.clone()),
                        window,
                        cx,
                    )
                })
            })
            .map(|target| (target, None)),
        Asked::LaneCreate {
            workspace,
            project,
            name,
            base_ref,
            agent,
            prompt,
        } => create_lane(
            (workspace, project),
            name,
            base_ref,
            pinned_chat(agent, mode),
            prompt,
            caller.origin(),
            cx,
        )
        .await
        .map(|(target, chat)| (chat, Some(target))),
    };
    let (chat, lane) = created?;
    // Read back rather than echo the request: what opened is the only honest
    // answer. The announcement keeps the command's delivery provenance.
    let label = cx.update(|cx| announce_chat(chat, caller, cx));
    let agent = label.map_or_else(String::new, |l| l.agent);
    Ok(match lane {
        Some(target) => ControlResult::LaneCreated {
            target,
            chat,
            agent,
        },
        None => ControlResult::ChatCreated {
            target: chat,
            agent,
        },
    })
}

/// The chat a card approved: the agent and the mode it named.
fn pinned_chat(agent: String, mode: daruda_config::ModeChoice) -> crate::workspace::ChatOpen {
    crate::workspace::ChatOpen {
        agent: crate::workspace::ChatAgent::Pinned(agent),
        mode,
    }
}

/// Name the new chat for remote channels, handing the label back for the
/// tool result. Announcing is best effort — see
/// `remote_channel::announce_chat_created` — and a pane that vanished in the
/// same tick has no label, so there is nothing to announce or report.
fn announce_chat(
    pane: PaneRef,
    caller: super::Caller,
    cx: &mut App,
) -> Option<crate::workspace::ChatLabel> {
    let delivery = caller.delivery(crate::app_presence::is_away(cx));
    let label = in_pane_window(pane, cx, |ws, _window, cx| {
        ws.control_chat_label(pane.pane, cx)
            .ok_or(ControlError::TargetGone)
    })
    .ok()?;
    crate::remote_channel::announce_chat_created(
        pane,
        label.path.clone(),
        &label.agent_name,
        &label.mode,
        delivery,
        cx,
    );
    Some(label)
}

/// Create a worktree, on whichever window holds the project.
///
/// The three-step sequence (plan, background git, register) lives in
/// `workspace::control_lane_ops` so its `CreateWorktreePlan` never leaves
/// `crate::workspace`; this only picks the window and waits.
async fn create_lane(
    project: (
        daruda_store::project::WorkspaceUuid,
        daruda_store::project::ProjectId,
    ),
    name: String,
    base_ref: Option<String>,
    chat: crate::workspace::ChatOpen,
    prompt: Option<String>,
    origin: super::CommandOrigin,
    cx: &mut gpui::AsyncApp,
) -> Result<(LaneHandle, PaneRef), ControlError> {
    let (workspace, project) = project;
    let rx = cx.update(|cx| {
        in_project_window(workspace, project, cx, |ws, window, cx| {
            Ok(ws.control_create_lane(
                project,
                name.clone(),
                base_ref.clone(),
                chat.clone(),
                window,
                cx,
            ))
        })
    })?;
    let (target, chat) = rx.recv().await.unwrap_or(Err(ControlError::TargetGone))?;
    let target = LaneHandle::new(workspace, target);

    if let Some(prompt) = prompt {
        // Best effort, and not its own outcome: the worktree exists either
        // way, and the caller can send the prompt again.
        cx.update(|cx| {
            let _ = in_pane_window(chat, cx, |ws, _window, cx| {
                ws.control_say(chat.pane, prompt.clone(), origin, cx)
            });
        });
    }
    Ok((target, chat))
}

#[cfg(test)]
mod tests;
