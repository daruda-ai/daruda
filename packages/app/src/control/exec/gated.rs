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
use crate::workspace::ChatDisclosure;

use super::{Dispatch, Pending, in_lane_window, in_pane_window, in_project_window, in_task_window};

mod card;
mod disclosure;
mod flow_resume;
mod tasks;

use card::{CardText, approval_summary};
pub(crate) use disclosure::{card_label, quoted_title};
use tasks::start_task;
pub(crate) use tasks::{begin_task_start, open_task, task_flow_summary};

/// A command as its card asked about it. A resume's run is resolved once,
/// before the card, so a handle such as `TaskLatest` cannot name another
/// execution by the time the user answers; no variant carries the handle.
/// The same holds for the agent a chat opens with: the card may wait for
/// minutes while the window's last agent changes, so it is a concrete id.
/// `shown` is the chat the card names, agent and mode as read then; the
/// approved work reads it again before anything is created (`still_shown`).
enum Asked {
    LaneCreate {
        workspace: daruda_store::project::WorkspaceUuid,
        project: daruda_store::project::ProjectId,
        name: String,
        base_ref: Option<String>,
        agent: String,
        prompt: Option<String>,
        shown: ChatDisclosure,
    },
    ChatNew {
        lane: LaneHandle,
        agent: String,
        shown: ChatDisclosure,
    },
    TaskCreate {
        workspace: daruda_store::project::WorkspaceUuid,
        project: daruda_store::project::ProjectId,
        task: NewTask,
    },
    /// `agent` is unused by a flow-backed task, whose nodes name their own;
    /// its `shown` is `None`.
    TaskStart {
        task: String,
        agent: String,
        shown: Option<ChatDisclosure>,
    },
    /// `shown` is the new pane a reopen opens, even while a pane still shows
    /// the run: that one may close before the answer. `None` when no new
    /// pane could load the run's session.
    TaskOpen {
        task: String,
        shown: Option<ChatDisclosure>,
    },
    FlowResume(flow_resume::ResumeTarget),
}

impl Asked {
    /// Resolve a resume's run and the agent a chat opens with, in the window
    /// that will open it, and how its card names that chat in `mode`; every
    /// other command is asked about as sent. Not resumable, gone or held by
    /// another run: no answer changes that, so the card is not shown.
    fn resolve(
        cmd: GatedCommand,
        mode: &daruda_config::ModeChoice,
        cx: &mut App,
    ) -> Result<Self, ControlError> {
        Ok(match cmd {
            GatedCommand::LaneCreate {
                workspace,
                project,
                name,
                base_ref,
                agent,
                prompt,
            } => {
                let agent = in_project_window(workspace, project, cx, |ws, _, _| {
                    Ok(ws.open_agent_id(agent.as_deref()))
                })?;
                Self::LaneCreate {
                    workspace,
                    project,
                    name,
                    base_ref,
                    shown: disclosure::in_project(workspace, project, &agent, mode, cx)?,
                    agent,
                    prompt,
                }
            }
            GatedCommand::ChatNew { lane, agent } => {
                let agent =
                    in_lane_window(lane, cx, |ws, _, _| Ok(ws.open_agent_id(agent.as_deref())))?;
                Self::ChatNew {
                    lane,
                    shown: disclosure::in_lane(lane, &agent, mode, cx)?,
                    agent,
                }
            }
            GatedCommand::TaskCreate {
                workspace,
                project,
                task,
            } => Self::TaskCreate {
                workspace,
                project,
                task,
            },
            GatedCommand::TaskStart { task } => {
                let agent = in_task_window(&task, cx, |ws, _, _| Ok(ws.open_agent_id(None)))?;
                Self::TaskStart {
                    shown: disclosure::task_start(&task, &agent, mode, cx)?,
                    agent,
                    task,
                }
            }
            GatedCommand::TaskOpen { task } => Self::TaskOpen {
                shown: disclosure::task_open(&task, mode, cx)?,
                task,
            },
            GatedCommand::FlowResume { run } => Self::FlowResume(flow_resume::target(&run, cx)?),
        })
    }

    /// Refuse an approval whose chat would no longer open as its card said:
    /// the agent's name or its mode in `mode` changed in settings, or its
    /// adapter renamed that mode, while the card waited. Read the same way
    /// `resolve` read it, and before anything is created.
    fn still_shown(
        &self,
        mode: &daruda_config::ModeChoice,
        cx: &mut App,
    ) -> Result<(), ControlError> {
        let unchanged = match self {
            Self::LaneCreate {
                workspace,
                project,
                agent,
                shown,
                ..
            } => disclosure::in_project(*workspace, *project, agent, mode, cx)? == *shown,
            Self::ChatNew { lane, agent, shown } => {
                disclosure::in_lane(*lane, agent, mode, cx)? == *shown
            }
            Self::TaskStart { task, agent, shown } => {
                disclosure::task_start(task, agent, mode, cx)? == *shown
            }
            Self::TaskOpen { task, shown } => disclosure::task_open(task, mode, cx)? == *shown,
            Self::TaskCreate { .. } | Self::FlowResume(_) => true,
        };
        if unchanged {
            Ok(())
        } else {
            Err(ControlError::ApprovalStale)
        }
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
    // The mode every chat this command opens takes, named on its card and
    // handed to the work as it is, so the two cannot disagree.
    let mode = crate::workspace::mode_for(origin.started_by());
    let asked = match Asked::resolve(cmd, &mode, cx) {
        Ok(asked) => asked,
        Err(refusal) => return Dispatch::Ready(Err(refusal)),
    };
    if let Err(refusal) = guard_gated(&asked, cx) {
        return Dispatch::Ready(Err(refusal));
    }
    let summary = match approval_summary(&asked, cx) {
        Ok(summary) => summary,
        Err(refusal) => return Dispatch::Ready(Err(refusal)),
    };
    let caller = origin.capture(cx);
    // Reserved before any read, so the call can be taken back while its card
    // is still being written.
    let (approval, answer) = crate::control::approval::reserve(cx);
    let reading = match summary {
        CardText::Now(summary) => {
            let delivery = caller.delivery(crate::app_presence::is_away(cx));
            crate::control::approval::ask(approval, summary, delivery, cx);
            None
        }
        CardText::Read(summary) => Some(summary),
    };
    let (tx, rx) = smol::channel::bounded(1);
    cx.spawn(async move |cx| {
        if let Some(summary) = reading {
            match summary.await {
                // Presence as it is once the card is ready to show, not as
                // it was before the read.
                Ok(summary) => cx.update(|cx| {
                    let delivery = caller.delivery(crate::app_presence::is_away(cx));
                    crate::control::approval::ask(approval, summary, delivery, cx);
                }),
                Err(refusal) => {
                    // Refused before the card, like the refusals above, unless
                    // the caller already took the call back and is owed nothing.
                    let owed = cx.update(|cx| crate::control::approval::abandon(approval, cx));
                    // SILENT-OK: the caller went away; nothing was asked or done.
                    let _ = tx.send(owed.then_some(Err(refusal))).await;
                    return;
                }
            }
        }
        let outcome: Option<ControlOutcome> = match answer.recv().await {
            // Approval may have waited while another creation spent a slot.
            // Reserve before the first await so overlapping approvals cannot
            // promise the same remaining worktree.
            Ok(ApprovalOutcome::Approved) => {
                Some(match cx.update(|cx| reserve_gated(&asked, &mode, cx)) {
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

/// Validate again, check the chat still opens as its card named it, and
/// claim a slot only for commands that create a worktree. A resume checks
/// its run again as it resumes.
fn reserve_gated(
    asked: &Asked,
    mode: &daruda_config::ModeChoice,
    cx: &mut App,
) -> Result<Option<crate::control::guards::CreationPermit>, ControlError> {
    if let Asked::FlowResume(_) = asked {
        return Ok(None);
    }
    guard_gated(asked, cx)?;
    asked.still_shown(mode, cx)?;
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
        Asked::TaskOpen { task, .. } => in_task_window(task, cx, |_ws, _window, _cx| Ok(())),
        // Resolved before the card: the run's state is read then, and once
        // more by the resume itself.
        Asked::FlowResume(_) => Ok(()),
    }
}

/// Whether a creation spent its slot: a success, or a refusal that names the
/// worktree git made — kept as a lane or left unregistered on disk. A
/// failure after git made one is always such a refusal; see
/// `TaskStartError::kept_in` and `TaskStartError::left_unregistered`.
fn left_a_worktree(outcome: &ControlOutcome) -> bool {
    matches!(
        outcome,
        Ok(_)
            | Err(ControlError::AgentUnavailableWorktreeKept { .. }
                | ControlError::TaskStartFailedWorktreeKept { .. }
                | ControlError::WorktreeUnregistered { .. })
    )
}

fn task_creates_lane(task: &daruda_store::tasks::TaskRecord) -> bool {
    matches!(
        task.spec.run_in,
        daruda_store::tasks::TaskRunIn::NewWorktree
    )
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
            return flow_resume::resume(target, caller, cx).await;
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
        // The start takes its mode from `caller`, as `mode` was taken from
        // its origin: `exec::origin`'s tests pin the two together.
        Asked::TaskStart { task, agent, .. } => {
            let agent = crate::workspace::ChatAgent::Pinned(agent);
            return start_task(task, agent, caller, cx).await;
        }
        Asked::TaskOpen { task, .. } => {
            return cx.update(|cx| tasks::open_task_in(task, mode, caller, cx));
        }
        Asked::ChatNew { lane, agent, .. } => cx
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
            ..
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
