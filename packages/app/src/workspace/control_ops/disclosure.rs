//! How a chat the control surface opens is disclosed to the person: the
//! agent, pinned before the approval card so the card names the agent that
//! opens, and the permission mode the chat asks its adapter for.

use crate::control::exec::card_label;
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
        ChatDisclosure {
            agent_name: card_label(agent.map_or(agent_id, |a| a.name.as_str())),
            mode: self.requested_mode_label(agent_id, &preferences),
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
    /// an adapter cannot relabel `bypassPermissions` as "Ask". Each name is
    /// an adapter's or a user's text, bounded by [`card_label`].
    pub(in crate::workspace) fn requested_mode_label(
        &self,
        agent_id: &str,
        preferences: &SessionPreferences,
    ) -> String {
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

    /// An adapter's own name for a mode stands beside the id when daruda's
    /// seed names it otherwise, and arrives flattened; one that agrees with
    /// the seed reads as the seed's. A user-set agent name is flattened too.
    #[gpui::test]
    fn an_adapter_cannot_relabel_the_requested_mode(cx: &mut gpui::TestAppContext) {
        let fixture = workspace_with_agent_chat(cx);
        fixture.workspace.update(cx, |ws, _| {
            let delegated = daruda_config::ModeChoice::FollowDelegated;
            let claude = daruda_config::AgentDefinition {
                name: "Claude\u{202E} Code\n".into(),
                ..daruda_config::AgentDefinition::claude_default()
            };
            let id = claude.id.clone();
            let source = claude.launch.adapter_command().to_owned();
            ws.mirrors.agents = vec![claude].into();
            let entry = |name: &str| daruda_store::agent_vocabulary::VocabEntry {
                id: "bypassPermissions".into(),
                name: name.into(),
                description: None,
            };
            ws.agent_vocabulary
                .record_modes(&id, &source, vec![entry("Bypass Permissions")]);
            let hidden = crate::surface::strings::control::approval_hidden_chars(1);
            let disclosure = ws.control_chat_disclosure(&id, &delegated);
            assert_eq!(disclosure.agent_name, format!("Claude Code{hidden}"));
            assert_eq!(disclosure.mode, "Bypass Permissions");
            ws.agent_vocabulary
                .record_modes(&id, &source, vec![entry("Ask\u{202E}\nfirst")]);
            assert_eq!(
                ws.control_chat_disclosure(&id, &delegated).mode,
                crate::surface::strings::control::approval_mode_adapter_named(
                    format!("Ask first{hidden}"),
                    "bypassPermissions"
                )
            );
        });
    }

    /// An agent name, an adapter's mode name and a user-set mode id are
    /// bounded as a title is, so none can swamp the card: both ends of a
    /// long one around how much was left out.
    #[gpui::test]
    fn long_agent_and_mode_names_are_cut_as_titles(cx: &mut gpui::TestAppContext) {
        use crate::surface::strings::control as s;
        use daruda_control_types::agent_text::{
            TITLE_EXCERPT_HEAD_CHARS as HEAD, TITLE_EXCERPT_TAIL_CHARS as TAIL,
        };
        let cut = |c: &str, n: usize| {
            format!(
                "{}{}{}",
                c.repeat(HEAD),
                s::approval_prompt_elided(n - HEAD - TAIL),
                c.repeat(TAIL)
            )
        };
        let fixture = workspace_with_agent_chat(cx);
        fixture.workspace.update(cx, |ws, _| {
            let delegated = daruda_config::ModeChoice::FollowDelegated;
            let claude = daruda_config::AgentDefinition {
                name: "n".repeat(300),
                delegated_mode: Some("i".repeat(300)),
                ..daruda_config::AgentDefinition::claude_default()
            };
            let id = claude.id.clone();
            let source = claude.launch.adapter_command().to_owned();
            ws.mirrors.agents = vec![claude].into();
            let disclosure = ws.control_chat_disclosure(&id, &delegated);
            assert_eq!(disclosure.agent_name, cut("n", 300));
            assert_eq!(disclosure.mode, cut("i", 300));
            ws.agent_vocabulary.record_modes(
                &id,
                &source,
                vec![daruda_store::agent_vocabulary::VocabEntry {
                    id: "i".repeat(300),
                    name: "a".repeat(400),
                    description: None,
                }],
            );
            assert_eq!(
                ws.control_chat_disclosure(&id, &delegated).mode,
                s::approval_mode_adapter_named(cut("a", 400), cut("i", 300))
            );
        });
    }
}
