//! The sentence an approval card asks with, one per `Asked` command. The
//! phrases it quotes are built in `disclosure`.

use gpui::App;

use crate::control::exec::{in_lane_window, in_project_window, in_task_window};
use crate::control::result::ControlError;
use crate::runs::GlobalTasks;
use crate::surface::strings as s;

use super::tasks::task_title;
use super::{Asked, disclosure, flow_resume, quoted_title, task_flow_summary};

/// A card's sentence: known now, or still being read off the UI thread.
pub(super) enum CardText {
    Now(String),
    /// Resolves to the sentence, or to a refusal what was read makes before
    /// any card is shown.
    Read(gpui::Task<Result<String, ControlError>>),
}

/// One line naming what the user is being asked to allow, built here because
/// `GatedCommand` carries no rendered sentences. A target that cannot be
/// named is refused rather than asked about. A chat's card names its pinned
/// agent and the mode `mode` resolves to; others' titles and prompts pass
/// through `agent_text` first.
pub(super) fn approval_summary(
    asked: &Asked,
    mode: &daruda_config::ModeChoice,
    cx: &mut App,
) -> Result<CardText, ControlError> {
    use disclosure::{agent_phrase, prompt_phrase};
    Ok(CardText::Now(match asked {
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
            // The guard already refused a name that hides or reorders text;
            // quoting it as well keeps the card safe if that rule changes.
            let name = quoted_title(name);
            match prompt.as_deref().and_then(prompt_phrase) {
                Some(prompt) => s::control::approval_lane_create_prompt(&name, agent, prompt),
                None => s::control::approval_lane_create(&name, agent),
            }
        }
        Asked::ChatNew { lane, agent } => {
            let (path, agent) = in_lane_window(*lane, cx, |ws, _window, _cx| {
                let path = quoted_title(&s::control::lane_path(
                    ws.control_project_name(lane.project),
                    ws.control_lane_name(lane.lane_ref()),
                ));
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
                        Some(lane) => s::control::approval_task_in_worktree(quoted_title(
                            &ws.control_lane_name(lane.lane_ref()),
                        )),
                        None => match task
                            .branch
                            .as_deref()
                            .map(str::trim)
                            .filter(|b| !b.is_empty())
                        {
                            Some(branch) => {
                                s::control::approval_task_in_new_branch(quoted_title(branch))
                            }
                            None => s::control::approval_task_in_new_worktree(),
                        },
                    };
                    let project = quoted_title(&ws.control_project_name(*project));
                    Ok((project, location, flow))
                })?;
            // The prompt is what the task's agents will be told once it
            // starts, which may be without another card (phone `/task`), so
            // it is quoted now unless the flow takes no input for it.
            let (surface, takes_prompt) = match flow {
                Some(flow) => (flow.summary, flow.takes_prompt),
                None => (s::control::approval_task_as_chat(), true),
            };
            let title = quoted_title(&task.title);
            match takes_prompt.then(|| prompt_phrase(&task.prompt)).flatten() {
                Some(prompt) => s::control::approval_task_create_prompt(
                    title, &name, &location, &surface, prompt,
                ),
                None => s::control::approval_task_create(title, &name, &location, &surface),
            }
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
                let (location, agent) = in_task_window(task, cx, |ws, _, cx| {
                    let location = cx
                        .global::<GlobalTasks>()
                        .get(task)
                        .map(|t| ws.control_task_start_location(t))
                        .ok_or(ControlError::TaskNotFound)?;
                    Ok((
                        location,
                        agent_phrase(&ws.control_chat_disclosure(agent, mode)),
                    ))
                })?;
                let title = task_title(task, cx);
                match prompt {
                    Some(prompt) => {
                        s::control::approval_task_start(title, &location, agent, prompt)
                    }
                    None => s::control::approval_task_start_no_prompt(title, &location, agent),
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
        Asked::FlowResume(target) => return flow_resume::summary(target, cx).map(CardText::Read),
    }))
}

/// `task`'s prompt as its start card quotes it; see `prompt_phrase`.
fn task_prompt(task: &str, cx: &App) -> Option<String> {
    cx.global::<GlobalTasks>()
        .get(task)
        .and_then(|t| disclosure::prompt_phrase(&t.spec.prompt))
}
