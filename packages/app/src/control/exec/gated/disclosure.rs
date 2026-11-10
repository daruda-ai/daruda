//! What an approval card discloses about the chat it would open: the agent,
//! the permission mode it asks for, its first prompt and the task's title. A
//! delegated chat approves its own tools, so a card naming only a title could
//! hide what the agent is about to be told. Prompts and titles are quoted
//! through `agent_text`, with this card's localized markers.

use daruda_config::ModeChoice;
use daruda_control_types::agent_text::{QuoteMarkers, prompt_excerpts, title_excerpt};
use gpui::App;

use crate::control::exec::{in_lane_window, in_project_window, in_task_window};
use crate::control::result::{ControlError, LaneHandle};
use crate::runs::GlobalTasks;
use crate::surface::strings as s;
use crate::workspace::ChatDisclosure;

// How each command's chat is disclosed, read from the window that opens it.
// A card reads it once; the approved work reads it again and compares what
// the chat asks for by id, so a settings change meanwhile cannot open a chat
// the card never named, while a name that only reads otherwise does not.

/// `agent`'s chat in a new worktree of `project`. `agent` is pinned: once
/// the catalog drops it, this refuses as the work would.
pub(super) fn in_project(
    workspace: daruda_store::project::WorkspaceUuid,
    project: daruda_store::project::ProjectId,
    agent: &str,
    mode: &ModeChoice,
    cx: &mut App,
) -> Result<ChatDisclosure, ControlError> {
    in_project_window(workspace, project, cx, |ws, _, _| {
        ws.control_pinned_chat_disclosure(agent, mode)
            .ok_or(ControlError::AgentUnavailable)
    })
}

/// `agent`'s chat in `lane`, pinned as for [`in_project`].
pub(super) fn in_lane(
    lane: LaneHandle,
    agent: &str,
    mode: &ModeChoice,
    cx: &mut App,
) -> Result<ChatDisclosure, ControlError> {
    in_lane_window(lane, cx, |ws, _, _| {
        ws.control_pinned_chat_disclosure(agent, mode)
            .ok_or(ControlError::AgentUnavailable)
    })
}

/// The chat a start of `task` opens under `agent`, pinned as for
/// [`in_project`]; `None` for a flow-backed task, whose nodes name their
/// own agents and modes.
pub(super) fn task_start(
    task: &str,
    agent: &str,
    mode: &ModeChoice,
    cx: &mut App,
) -> Result<Option<ChatDisclosure>, ControlError> {
    let record = cx
        .global::<GlobalTasks>()
        .get(task)
        .ok_or(ControlError::TaskNotFound)?;
    if record.spec.flow.is_some() {
        return Ok(None);
    }
    in_task_window(task, cx, |ws, _, _| {
        ws.control_pinned_chat_disclosure(agent, mode)
            .map(Some)
            .ok_or(ControlError::AgentUnavailable)
    })
}

/// The new pane a reopen of `task`'s chat opens: now, or once the pane
/// still showing it closes. `None` when no new pane could load its session.
/// The run's agent is not pinned by a card; a removed one reads as its id.
pub(super) fn task_open(
    task: &str,
    mode: &ModeChoice,
    cx: &mut App,
) -> Result<Option<ChatDisclosure>, ControlError> {
    in_task_window(task, cx, |ws, _, cx| {
        Ok(ws
            .task_chat_new_pane_agent(task, cx)
            .map(|agent| ws.control_chat_disclosure(&agent, mode)))
    })
}

/// The markers a card or phone row quotes text with, in the reader's locale.
struct Localized;

impl QuoteMarkers for Localized {
    fn quote(&self, text: &str) -> String {
        s::control::approval_quoted(text)
    }

    fn elided(&self, n: usize) -> String {
        s::control::approval_prompt_elided(n)
    }

    fn hidden(&self, n: usize) -> String {
        s::control::approval_hidden_chars(n)
    }
}

/// A title someone else wrote, as a card or a phone row quotes it: one quoted
/// line, both ends of a long one, and a count of what the card cannot show.
/// Branch and worktree names on a card are quoted the same way.
pub(crate) fn quoted_title(title: &str) -> String {
    title_excerpt(title, &Localized)
}

/// [`Localized`], without the quotation marks: for a name the card states
/// as its own words, such as an agent's or a mode's.
struct Unquoted;

impl QuoteMarkers for Unquoted {
    fn quote(&self, text: &str) -> String {
        text.to_owned()
    }

    fn elided(&self, n: usize) -> String {
        Localized.elided(n)
    }

    fn hidden(&self, n: usize) -> String {
        Localized.hidden(n)
    }
}

/// A name the card states rather than quotes — an agent's display name, an
/// adapter's name for a mode, a user-set mode id — bounded as a title is:
/// one line, both ends of a long one, and a count of what it dropped.
pub(crate) fn card_label(label: &str) -> String {
    title_excerpt(label, &Unquoted)
}

/// "Claude Code (requested permission mode: Bypass Permissions)".
pub(super) fn agent_phrase(disclosure: &ChatDisclosure) -> String {
    s::control::approval_agent(&disclosure.agent_name, &disclosure.mode)
}

/// The most a card's sentence may hold, in UTF-16 code units: Telegram's
/// `sendMessage` takes 4096 characters and sends the card as one message,
/// where Slack and Discord split what exceeds their limits. UTF-16 is the
/// stricter unit, as in `remote_channel::transport::chunks`. The rest is
/// room for the outcome a settled card is edited to append.
pub(super) const CARD_MAX_UTF16: usize = 3900;

/// Whether a card's sentence fits one phone message.
pub(super) fn fits_one_message(card: &str) -> bool {
    card.encode_utf16().count() <= CARD_MAX_UTF16
}

/// The card `sentence` makes of `prompt`: one quoted line, its start and end
/// around how much was left out, at the widest bound that keeps the card in
/// one phone message; the floor bound when none does, which the other
/// fields' own bounds leave room for. `None` when the prompt carries
/// nothing, so the card says it sends no prompt rather than quoting an
/// empty one.
pub(super) fn with_prompt(prompt: &str, sentence: impl Fn(&str) -> String) -> Option<String> {
    let mut narrowest = None;
    for excerpt in prompt_excerpts(prompt, &Localized) {
        if excerpt.is_empty() {
            return None;
        }
        let card = sentence(&excerpt);
        if fits_one_message(&card) {
            return Some(card);
        }
        narrowest = Some(card);
    }
    narrowest
}

#[cfg(test)]
mod tests;
