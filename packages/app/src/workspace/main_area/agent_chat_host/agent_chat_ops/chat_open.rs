//! The chat a lane, a task or a control command opens: which agent, and
//! where its session mode comes from.

use daruda_config::{AgentDefinition, ModeChoice};
use daruda_store::tasks::StartedBy;

use super::resolve_open_agent_id;
use crate::workspace::Workspace;

/// Where a chat's session mode comes from, by who asked for it. The phone
/// and the orchestrator get the agent's delegated mode, since nobody at the
/// desktop answers their chat's permission prompts; a person at the desktop
/// gets the agent's default mode. The one place this split is made.
pub(crate) fn mode_for(started_by: StartedBy) -> ModeChoice {
    match started_by {
        StartedBy::Desktop => ModeChoice::FollowDefault,
        StartedBy::Phone | StartedBy::Orchestrator => ModeChoice::FollowDelegated,
    }
}

/// Which agent a new chat opens under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChatAgent {
    /// This agent while the catalog holds it, else the window's last agent,
    /// else the catalog default.
    Preferred(Option<String>),
    /// An agent a person approved on a card. Refused once it is gone: any
    /// other would open in an agent and mode that person never saw.
    Pinned(String),
}

/// A pinned agent the catalog no longer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentUnavailable;

/// The chat a lane or task opens with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChatOpen {
    pub(crate) agent: ChatAgent,
    pub(crate) mode: ModeChoice,
}

impl ChatOpen {
    /// A chat no card pinned an agent for, in the mode `started_by` gets.
    pub(crate) fn preferred(started_by: StartedBy) -> Self {
        Self {
            agent: ChatAgent::Preferred(None),
            mode: mode_for(started_by),
        }
    }
}

#[cfg(test)]
impl ChatOpen {
    /// A chat the orchestrator asks for, naming no agent.
    pub(crate) fn orchestrator_for_test() -> Self {
        Self {
            agent: ChatAgent::Preferred(None),
            mode: mode_for(StartedBy::Orchestrator),
        }
    }
}

impl Workspace {
    /// The concrete agent `agent` opens, read from the catalog now. Called
    /// where the pane is built, so an agent removed after an earlier check
    /// is still refused rather than replaced.
    pub(in crate::workspace) fn resolve_chat_agent(
        &self,
        agent: &ChatAgent,
    ) -> Result<String, AgentUnavailable> {
        resolve_chat_agent(&self.mirrors.agents, self.last_agent_id.as_deref(), agent)
    }
}

/// Pure core of [`Workspace::resolve_chat_agent`].
fn resolve_chat_agent(
    agents: &[AgentDefinition],
    last: Option<&str>,
    agent: &ChatAgent,
) -> Result<String, AgentUnavailable> {
    match agent {
        ChatAgent::Preferred(id) => Ok(resolve_open_agent_id(agents, id.as_deref().or(last))),
        ChatAgent::Pinned(id) if agents.iter().any(|a| a.id == *id) => Ok(id.clone()),
        ChatAgent::Pinned(_) => Err(AgentUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daruda_config::AgentLaunch;

    fn catalog() -> Vec<AgentDefinition> {
        vec![
            AgentDefinition::claude_default(),
            AgentDefinition::new(
                "other".to_string(),
                "Other".to_string(),
                AgentLaunch::Raw("run-other".to_string()),
            ),
        ]
    }

    #[test]
    fn only_a_desktop_request_follows_the_default_mode() {
        assert_eq!(mode_for(StartedBy::Desktop), ModeChoice::FollowDefault);
        for remote in [StartedBy::Phone, StartedBy::Orchestrator] {
            assert_eq!(mode_for(remote), ModeChoice::FollowDelegated);
        }
    }

    /// A pinned agent opens only itself; a preferred one falls back as a
    /// new pane always has.
    #[test]
    fn a_removed_pinned_agent_is_refused_and_a_preferred_one_falls_back() {
        let agents = catalog();
        let claude = AgentDefinition::claude_default().id;
        let pinned = |id: &str| ChatAgent::Pinned(id.to_owned());
        assert_eq!(
            resolve_chat_agent(&agents, None, &pinned("other")),
            Ok("other".into())
        );
        assert_eq!(
            resolve_chat_agent(&agents, Some("other"), &pinned("gone")),
            Err(AgentUnavailable)
        );
        let preferred = |id: Option<&str>| ChatAgent::Preferred(id.map(str::to_owned));
        assert_eq!(
            resolve_chat_agent(&agents, Some("other"), &preferred(Some("gone"))),
            Ok(claude.clone())
        );
        assert_eq!(
            resolve_chat_agent(&agents, Some("other"), &preferred(None)),
            Ok("other".into())
        );
    }
}
