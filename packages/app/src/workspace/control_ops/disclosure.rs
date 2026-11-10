//! How a chat the control surface opens is disclosed to the person: the
//! agent, pinned before the approval card so the card names the agent that
//! opens, and the permission mode the chat asks its adapter for.

use crate::control::exec::card_label;
use daruda_config::{ModeChoice, SessionPreferences};

use crate::workspace::Workspace;

/// A chat's agent and requested permission mode, as a person reads them,
/// and by id as the chat will ask for them.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChatDisclosure {
    pub(crate) agent_name: String,
    pub(crate) mode: String,
    pub(crate) requested: RequestedChat,
}

/// What a chat asks for, by id: the agent and its permission-mode
/// candidates in priority order. An approval compares this rather than the
/// card's words, which change when a connecting adapter names its modes
/// although the chat asks for the same one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestedChat {
    pub(crate) agent_id: String,
    pub(crate) modes: Vec<String>,
}

impl Workspace {
    /// How a card names a chat of `agent_id` whose mode comes from `mode`:
    /// its display name and the mode the pane will ask for, by the same rule
    /// its connect uses. The name is user-set text, bounded by
    /// [`card_label`] so it cannot reorder, break or swamp the card's
    /// sentence.
    pub(crate) fn control_chat_disclosure(
        &self,
        agent_id: &str,
        mode: &ModeChoice,
    ) -> ChatDisclosure {
        let agent = self.mirrors.agents.iter().find(|a| a.id == agent_id);
        let preferences = SessionPreferences::resolve(agent, None, mode);
        let name = agent.map_or(agent_id, |a| a.name.as_str());
        self.chat_disclosure_of(agent_id, name, preferences)
    }

    /// How a card or phone text names a chat of `agent_id`, called `name`,
    /// that asks for `preferences`.
    pub(super) fn chat_disclosure_of(
        &self,
        agent_id: &str,
        name: &str,
        preferences: SessionPreferences,
    ) -> ChatDisclosure {
        ChatDisclosure {
            agent_name: card_label(name),
            mode: self.requested_mode_label(agent_id, &preferences),
            requested: RequestedChat {
                agent_id: agent_id.to_owned(),
                modes: preferences.modes,
            },
        }
    }

    /// [`Self::control_chat_disclosure`] for an agent a card pinned, or
    /// `None` once the catalog no longer holds it: the work refuses that as
    /// `agent_unavailable`, a cause more precise than a changed card.
    pub(crate) fn control_pinned_chat_disclosure(
        &self,
        agent_id: &str,
        mode: &ModeChoice,
    ) -> Option<ChatDisclosure> {
        self.mirrors
            .agents
            .iter()
            .any(|a| a.id == agent_id)
            .then(|| self.control_chat_disclosure(agent_id, mode))
    }

    /// The display name of the first mode `preferences` asks for, as
    /// [`daruda_config::mode_display_name`] names it for `agent_id`'s adapter.
    /// The first candidate is the one requested; a connect falls through
    /// only when it is not advertised. An adapter advertises its own names,
    /// so one daruda's seed does not match is shown beside the mode's id:
    /// an adapter cannot relabel `bypassPermissions` as "Ask".
    pub(in crate::workspace) fn requested_mode_label(
        &self,
        agent_id: &str,
        preferences: &SessionPreferences,
    ) -> String {
        // Every name below is an adapter's or a user's text, so each is
        // bounded by `card_label`.
        let Some(mode) = preferences.modes.first() else {
            return crate::surface::strings::control::approval_mode_agent_default();
        };
        let Some(agent) = self.mirrors.agents.iter().find(|a| a.id == agent_id) else {
            return card_label(mode);
        };
        let source = agent.launch.adapter_command().trim();
        let known = self.agent_vocabulary.known_modes_for(agent_id, source);
        let named = daruda_config::mode_display_name(known, source, mode);
        // The seed's name, or the id itself where the seed has none.
        if named == daruda_config::mode_display_name(None, source, mode) {
            return card_label(&named);
        }
        crate::surface::strings::control::approval_mode_adapter_named(
            card_label(&named),
            card_label(mode),
        )
    }
}

#[cfg(test)]
mod tests;
