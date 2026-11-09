//! How a chat the control surface opens is disclosed to the person: the
//! agent, pinned before the approval card so the card names the agent that
//! opens, and the permission mode the chat asks its adapter for.

use daruda_config::{ModeChoice, SessionPreferences};

use crate::workspace::Workspace;

/// A chat's agent and requested permission mode, as a person reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatDisclosure {
    pub(crate) agent_name: String,
    pub(crate) mode: String,
}

impl Workspace {
    /// How a card names a chat of `agent_id` whose mode comes from `mode`:
    /// its display name and the mode the pane will ask for, by the same rule
    /// its connect uses.
    pub(crate) fn control_chat_disclosure(
        &self,
        agent_id: &str,
        mode: &ModeChoice,
    ) -> ChatDisclosure {
        let agent = self.mirrors.agents.iter().find(|a| a.id == agent_id);
        let preferences = SessionPreferences::resolve(agent, None, mode);
        ChatDisclosure {
            agent_name: agent.map_or_else(|| agent_id.to_owned(), |a| a.name.clone()),
            mode: self.requested_mode_label(agent_id, &preferences),
        }
    }

    /// The display name of the first mode `preferences` asks for, as
    /// [`daruda_config::mode_display_name`] names it for `agent_id`'s adapter.
    /// The first candidate is the one requested; a connect falls through
    /// only when it is not advertised.
    pub(in crate::workspace) fn requested_mode_label(
        &self,
        agent_id: &str,
        preferences: &SessionPreferences,
    ) -> String {
        let Some(mode) = preferences.modes.first() else {
            return crate::surface::strings::control::approval_mode_agent_default();
        };
        let Some(agent) = self.mirrors.agents.iter().find(|a| a.id == agent_id) else {
            return mode.clone();
        };
        let source = agent.launch.adapter_command().trim();
        let known = self.agent_vocabulary.known_modes_for(agent_id, source);
        daruda_config::mode_display_name(known, source, mode)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::workspace_with_agent_chat;

    /// The card names Claude's delegated default by the adapter's own label,
    /// even once the adapter advertised no modes; an id no vocabulary names
    /// by the id itself; and an agent with no mode at all by the wording for
    /// the agent's own default.
    #[gpui::test]
    fn the_delegated_mode_is_named_from_the_vocabulary(cx: &mut gpui::TestAppContext) {
        let fixture = workspace_with_agent_chat(cx);
        fixture.workspace.update(cx, |ws, _| {
            let delegated = daruda_config::ModeChoice::FollowDelegated;
            let claude = daruda_config::AgentDefinition::claude_default();
            let id = claude.id.clone();
            ws.mirrors.agents = vec![
                claude.clone(),
                daruda_config::AgentDefinition {
                    id: "custom".into(),
                    delegated_mode: Some("yolo".into()),
                    ..claude.clone()
                },
                daruda_config::AgentDefinition {
                    id: "plain".into(),
                    launch: daruda_config::AgentLaunch::Raw("plain-acp".into()),
                    default_mode: None,
                    delegated_mode: None,
                    ..claude.clone()
                },
            ]
            .into();
            let disclosure = ws.control_chat_disclosure(&id, &delegated);
            assert_eq!(disclosure.agent_name, claude.name);
            assert_eq!(disclosure.mode, "Bypass Permissions");
            // The same agent opened for a person asks for its ordinary mode.
            assert_ne!(
                ws.control_chat_disclosure(&id, &daruda_config::ModeChoice::FollowDefault)
                    .mode,
                "Bypass Permissions"
            );
            let source = claude.launch.adapter_command().to_owned();
            ws.agent_vocabulary.record_modes(&id, &source, Vec::new());
            assert_eq!(
                ws.control_chat_disclosure(&id, &delegated).mode,
                "Bypass Permissions"
            );
            assert_eq!(
                ws.control_chat_disclosure("custom", &delegated).mode,
                "yolo"
            );
            assert_eq!(
                ws.control_chat_disclosure("plain", &delegated).mode,
                crate::surface::strings::control::approval_mode_agent_default()
            );
        });
    }
}
