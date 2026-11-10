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
/// named is refused rather than asked about. A chat's card names the agent
/// and mode `Asked` carries as `shown`; others' titles and prompts pass
/// through `agent_text` first.
pub(super) fn approval_summary(asked: &Asked, cx: &mut App) -> Result<CardText, ControlError> {
    use disclosure::{agent_phrase, with_prompt};
    Ok(CardText::Now(match asked {
        Asked::LaneCreate {
            name,
            base_ref,
            prompt,
            shown,
            ..
        } => {
            let agent = agent_phrase(shown);
            // The guard already refused a name that hides or reorders text;
            // quoting it as well keeps the card safe if that rule changes.
            let name = quoted_title(name);
            // The base decides what code the agent starts from, so a
            // requested one is named; none branches from the project's own.
            let name = match base_ref.as_deref().filter(|base| !base.trim().is_empty()) {
                Some(base) => s::control::approval_lane_on_base(&name, quoted_title(base)),
                None => name,
            };
            prompt
                .as_deref()
                .and_then(|prompt| {
                    with_prompt(prompt, |prompt| {
                        s::control::approval_lane_create_prompt(&name, &agent, prompt)
                    })
                })
                .unwrap_or_else(|| s::control::approval_lane_create(&name, &agent))
        }
        Asked::ChatNew { lane, shown, .. } => {
            let path = in_lane_window(*lane, cx, |ws, _window, _cx| {
                Ok(quoted_title(&s::control::lane_path(
                    ws.control_project_name(lane.project),
                    ws.control_lane_name(lane.lane_ref()),
                )))
            })?;
            s::control::approval_chat_new(&path, agent_phrase(shown))
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
            takes_prompt
                .then(|| {
                    with_prompt(&task.prompt, |prompt| {
                        s::control::approval_task_create_prompt(
                            &title, &name, &location, &surface, prompt,
                        )
                    })
                })
                .flatten()
                .unwrap_or_else(|| {
                    s::control::approval_task_create(&title, &name, &location, &surface)
                })
        }
        // A flow given the prompt hands it to its agents, so its card quotes
        // it as a chat's does.
        Asked::TaskStart { task, shown, .. } => match shown {
            None => {
                let flow = task_flow_summary(task, cx).ok_or(ControlError::TaskNotFound)?;
                let location = task_start_location(task, cx)?;
                let title = task_title(task, cx);
                flow.takes_prompt
                    .then(|| {
                        with_prompt(&task_prompt(task, cx), |prompt| {
                            s::control::approval_task_start_flow_prompt(
                                &title,
                                &location,
                                &flow.summary,
                                prompt,
                            )
                        })
                    })
                    .flatten()
                    .unwrap_or_else(|| {
                        s::control::approval_task_start_flow(&title, &location, &flow.summary)
                    })
            }
            Some(shown) => {
                let prompt = task_prompt(task, cx);
                let location = task_start_location(task, cx)?;
                let agent = agent_phrase(shown);
                let title = task_title(task, cx);
                with_prompt(&prompt, |prompt| {
                    s::control::approval_task_start(&title, &location, &agent, prompt)
                })
                .unwrap_or_else(|| {
                    s::control::approval_task_start_no_prompt(&title, &location, &agent)
                })
            }
        },
        // A chat already open keeps its own mode, but may close before the
        // answer, so the card names the new pane's mode either way; the
        // open follows the pane as it finds it.
        Asked::TaskOpen { task, shown } => {
            let open = in_task_window(task, cx, |ws, _, cx| Ok(ws.task_chat_pane_open(task, cx)))?;
            let title = task_title(task, cx);
            match shown.as_ref().map(agent_phrase) {
                Some(agent) if open => s::control::approval_task_open_existing(title, agent),
                Some(agent) => s::control::approval_task_open_new(title, agent),
                None => s::control::approval_task_open(title),
            }
        }
        Asked::FlowResume(target) => return flow_resume::summary(target, cx).map(CardText::Read),
    }))
}

/// Where a start of `task` runs, as its card names it.
fn task_start_location(task: &str, cx: &mut App) -> Result<String, ControlError> {
    in_task_window(task, cx, |ws, _, cx| {
        cx.global::<GlobalTasks>()
            .get(task)
            .map(|t| ws.control_task_start_location(t))
            .ok_or(ControlError::TaskNotFound)
    })
}

/// `task`'s prompt, which its start card quotes through `with_prompt`;
/// empty for a task that is gone, which quotes as no prompt.
fn task_prompt(task: &str, cx: &App) -> String {
    cx.global::<GlobalTasks>()
        .get(task)
        .map(|t| t.spec.prompt.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::disclosure::{card_label, fits_one_message, with_prompt};
    use super::quoted_title;
    use crate::surface::strings::control as s;

    /// Text at its worst for a card: astral characters, two UTF-16 units
    /// each, long enough to be cut, with invisible ones to count.
    fn hostile() -> String {
        format!("{}{}", "😀".repeat(5000), "\u{200B}".repeat(99_999))
    }

    /// Every field beside the prompt is bounded, so even at its worst a card
    /// leaves the prompt room for its floor excerpt within one phone
    /// message, and the prompt takes the widest that fits.
    #[test]
    fn the_longest_card_still_fits_one_phone_message() {
        let quoted = quoted_title(&hostile());
        let label = card_label(&hostile());
        let agent = s::approval_agent(&label, s::approval_mode_adapter_named(&label, &label));
        let location = s::approval_task_in_worktree_on_branch(&quoted, &quoted);
        let from_base = s::approval_task_in_new_branch_from(&quoted, &quoted);
        // A flow's modes: four quoted, the rest counted.
        let modes = s::approval_flow_modes(format!(
            "{}, {}",
            [quoted.as_str(); 4].join(", "),
            s::approval_flow_modes_more(99_999)
        ));
        let surface = s::approval_task_as_flow_profile(&quoted, &quoted, &modes);
        let prompt = hostile();
        let cards: [&dyn Fn(&str) -> String; 5] = [
            &|p| s::approval_task_create_prompt(&quoted, &quoted, &location, &surface, p),
            &|p| s::approval_task_start_flow_prompt(&quoted, &from_base, &surface, p),
            &|p| s::approval_task_start(&quoted, &location, &agent, p),
            &|p| s::approval_task_start(&quoted, &from_base, &agent, p),
            &|p| {
                s::approval_lane_create_prompt(
                    s::approval_lane_on_base(&quoted, &quoted),
                    &agent,
                    p,
                )
            },
        ];
        for card in cards {
            let shown = with_prompt(&prompt, card).expect("a prompt");
            assert!(
                fits_one_message(&shown),
                "{} units: {shown}",
                shown.encode_utf16().count()
            );
        }
    }
}
