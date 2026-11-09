//! How a chat the control surface opens is disclosed to the person: the
//! agent, pinned before the approval card so the card names the agent that
//! opens, and the permission mode the chat asks its adapter for.

use daruda_config::{AgentDefinition, ModeChoice, SessionPreferences};

use crate::workspace::Workspace;

/// A chat's agent and requested permission mode, as a person reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatDisclosure {
    pub(crate) agent_name: String,
    pub(crate) mode: String,
}

impl Workspace {
    /// The agent a chat the control surface opens runs under: `requested`
    /// while the catalog holds it, else the window's last agent, else the
    /// catalog default. Resolved once, before the card, and passed to the
    /// open as a concrete id, so a later agent switch cannot change what the
    /// person approved.
    pub(crate) fn control_open_agent_id(&self, requested: Option<&str>) -> String {
        self.open_agent_id(requested)
    }

    /// How a card names a delegated chat of `agent_id`: its display name and
    /// the mode the pane will ask for, by the same rule its connect uses.
    pub(crate) fn control_delegated_disclosure(&self, agent_id: &str) -> ChatDisclosure {
        let agent = self.mirrors.agents.iter().find(|a| a.id == agent_id);
        let preferences = SessionPreferences::resolve(agent, None, &ModeChoice::FollowDelegated);
        ChatDisclosure {
            agent_name: agent.map_or_else(|| agent_id.to_owned(), |a| a.name.clone()),
            mode: self.requested_mode_label(agent_id, agent, &preferences),
        }
    }

    /// The display name of the first mode `preferences` asks for, as the
    /// agent's vocabulary names it: what it last advertised, else the
    /// adapter seed, else the id itself. The first candidate is the one
    /// requested; a connect falls through only when it is not advertised.
    /// Unlike a picker's options, a name is looked up in both lists: an
    /// adapter that advertised no modes does not make the seed's name wrong.
    pub(in crate::workspace) fn requested_mode_label(
        &self,
        agent_id: &str,
        agent: Option<&AgentDefinition>,
        preferences: &SessionPreferences,
    ) -> String {
        let Some(mode) = preferences.modes.first() else {
            return crate::surface::strings::control::approval_mode_agent_default();
        };
        let Some(agent) = agent else {
            return mode.clone();
        };
        let source = crate::lane::session_host::adapter_command(&agent.launch).trim();
        let seed = daruda_config::agent_vocabulary_seed(source);
        let known = self.agent_vocabulary.known_modes_for(agent_id, source);
        let seeded = seed.as_ref().map(|seed| seed.modes.as_slice());
        [known, seeded]
            .into_iter()
            .flatten()
            .flatten()
            .find(|entry| entry.id == *mode)
            .map_or_else(|| mode.clone(), |entry| entry.name.clone())
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
            let disclosure = ws.control_delegated_disclosure(&id);
            assert_eq!(disclosure.agent_name, claude.name);
            assert_eq!(disclosure.mode, "Bypass Permissions");
            let source = crate::lane::session_host::adapter_command(&claude.launch).to_owned();
            ws.agent_vocabulary.record_modes(&id, &source, Vec::new());
            assert_eq!(
                ws.control_delegated_disclosure(&id).mode,
                "Bypass Permissions"
            );
            assert_eq!(ws.control_delegated_disclosure("custom").mode, "yolo");
            assert_eq!(
                ws.control_delegated_disclosure("plain").mode,
                crate::surface::strings::control::approval_mode_agent_default()
            );
        });
    }
}
