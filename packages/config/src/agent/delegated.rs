//! The mode a session runs in when daruda opens it because the phone or the
//! orchestrator asked, rather than the person at the desktop.
//!
//! Policy, not a fact about the adapter: nobody is at the desktop to answer a
//! permission prompt, so the default is the adapter's own no-prompt mode. That
//! is why it lives here rather than in the [`vocabulary`](super::vocabulary)
//! seed, which states only what the adapter itself does.

use daruda_store::accounts::AccountRecipeId;

use super::{AgentDefinition, AgentLaunch, account_recipe_for_local_command};

/// Claude adapter's no-prompt mode; one of the seed's `CLAUDE_MODES`.
const CLAUDE_DELEGATED_MODE: &str = "bypassPermissions";

/// Codex adapter's no-prompt mode; one of the seed's `CODEX_MODES`.
const CODEX_DELEGATED_MODE: &str = "agent-full-access";

/// The mode a phone- or orchestrator-requested session runs in when the agent
/// names none: the adapter's own no-prompt mode. `None` for an adapter daruda
/// knows no such mode for, which leaves the agent's `default_mode` standing.
///
/// Keyed on the adapter command whatever host it runs on: the mode is the
/// adapter's vocabulary, and an SSH or Docker launch runs the same adapter.
pub fn delegated_default(launch: &AgentLaunch) -> Option<&'static str> {
    match account_recipe_for_local_command(launch.adapter_command())? {
        AccountRecipeId::Claude => Some(CLAUDE_DELEGATED_MODE),
        AccountRecipeId::Codex => Some(CODEX_DELEGATED_MODE),
    }
}

/// The agent's own `delegated_mode` when it states a non-blank one, else
/// [`delegated_default`].
pub fn effective_delegated_mode(agent: &AgentDefinition) -> Option<String> {
    agent
        .delegated_mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
        .or_else(|| delegated_default(&agent.launch))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::vocabulary::seed_for_command;

    /// A policy default the adapter does not advertise would be skipped at
    /// connect, silently leaving a delegated session on prompting modes.
    #[test]
    fn each_policy_default_is_in_its_adapters_vocabulary() {
        for command in [
            "npx -y @agentclientprotocol/claude-agent-acp@latest",
            "npx codex-acp",
        ] {
            let launch = AgentLaunch::Raw(command.to_string());
            let mode = delegated_default(&launch).expect("a known adapter");
            let seed = seed_for_command(command).expect("a seeded adapter");
            assert!(seed.modes.iter().any(|entry| entry.id == mode), "{mode}");
        }
    }
}
