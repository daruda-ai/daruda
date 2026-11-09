//! Build-time mode / model vocabularies for the ACP adapters daruda ships
//! facts about — what a picker offers for an agent that has never connected,
//! before [`daruda_store::agent_vocabulary`] has recorded anything real.
//!
//! Keyed on adapter identity via [`account_recipe_for_local_command`] rather
//! than agent id, so a hand-written catalog entry launching the same adapter
//! gets the same seed.

use daruda_store::accounts::AccountRecipeId;
use daruda_store::agent_vocabulary::VocabEntry;

use super::account_recipe_for_local_command;

/// One adapter's statically-known vocabulary.
pub struct AgentVocabularySeed {
    pub modes: Vec<VocabEntry>,
    pub models: Vec<VocabEntry>,
    /// What the adapter uses when nothing is pinned. Display only —
    /// never sent at connect; both adapters already clamp to it themselves.
    pub default_mode: Option<String>,
    pub default_model: Option<String>,
}

/// The vocabulary of the adapter `command` launches. `None` when the
/// command names no adapter daruda has facts about.
pub fn seed_for_command(command: &str) -> Option<AgentVocabularySeed> {
    match account_recipe_for_local_command(command)? {
        AccountRecipeId::Claude => Some(claude_seed()),
        AccountRecipeId::Codex => Some(codex_seed()),
    }
}

/// The display name of mode `id` for the adapter `command` launches: what
/// the agent last advertised (`known`), else the adapter seed, else `id`.
/// Looked up in both lists, unlike a picker's options: an adapter that
/// advertised no modes, or not this one, does not make the seed's name wrong.
pub fn mode_display_name(known: Option<&[VocabEntry]>, command: &str, id: &str) -> String {
    let seed = seed_for_command(command);
    let seeded = seed.as_ref().map(|seed| seed.modes.as_slice());
    [known, seeded]
        .into_iter()
        .flatten()
        .flatten()
        .find(|entry| entry.id == id)
        .map_or_else(|| id.to_string(), |entry| entry.name.clone())
}

/// `@agentclientprotocol/claude-agent-acp@0.70.0`, `dist/acp-agent.js:5393`
/// (`buildAvailableModes`). `auto` is advertised only for a model reporting
/// `supportsAutoMode` and `bypassPermissions` only off root; both are listed
/// anyway, because an unadvertised candidate is simply skipped at connect.
const CLAUDE_MODES: &[(&str, &str)] = &[
    ("auto", "Auto"),
    ("default", "Manual"),
    ("acceptEdits", "Accept Edits"),
    ("plan", "Plan Mode"),
    ("dontAsk", "Don't Ask"),
    ("bypassPermissions", "Bypass Permissions"),
];

/// The model aliases the same adapter resolves itself (`acp-agent.js:3781`,
/// `:5601`, `:5725`) — the real per-account list only arrives on connect.
/// Names are daruda's own: the adapter takes these as input rather than
/// advertising them, so it states no display name for them.
const CLAUDE_MODELS: &[(&str, &str)] = &[
    ("default", "Default"),
    ("opus", "Opus"),
    ("sonnet", "Sonnet"),
    ("opusplan", "Opus Plan"),
];

/// `resolvePermissionMode(undefined)`, `acp-agent.js:407-410`.
const CLAUDE_DEFAULT_MODE: &str = "default";

/// The `"default"` model id the adapter resolves against the account's plan.
const CLAUDE_DEFAULT_MODEL: &str = "default";

/// `@agentclientprotocol/codex-acp@1.1.0`, `dist/index.js:24849` (`AgentMode`).
const CODEX_MODES: &[(&str, &str)] = &[
    ("read-only", "Read-only"),
    ("agent", "Agent"),
    ("agent-full-access", "Agent (full access)"),
];

/// `DEFAULT_AGENT_MODE`, `index.js:24897`.
const CODEX_DEFAULT_MODE: &str = "agent";

fn claude_seed() -> AgentVocabularySeed {
    AgentVocabularySeed {
        modes: entries(CLAUDE_MODES),
        models: entries(CLAUDE_MODELS),
        default_mode: Some(CLAUDE_DEFAULT_MODE.to_string()),
        default_model: Some(CLAUDE_DEFAULT_MODEL.to_string()),
    }
}

/// Codex seeds no models: its list comes from a backend `listModels` call
/// (`index.js:25757`, `createModelConfigOption`), so there is no id daruda
/// can state at build time — and none it may invent.
fn codex_seed() -> AgentVocabularySeed {
    AgentVocabularySeed {
        modes: entries(CODEX_MODES),
        models: Vec::new(),
        default_mode: Some(CODEX_DEFAULT_MODE.to_string()),
        default_model: None,
    }
}

fn entries(table: &[(&str, &str)]) -> Vec<VocabEntry> {
    table
        .iter()
        .map(|(id, name)| VocabEntry::new(*id, *name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(entries: &[VocabEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    /// The seed is resolved from adapter markers, not from an agent id, so a
    /// hand-written entry running the same adapter gets the same vocabulary.
    #[test]
    fn the_seed_follows_the_adapter_marker_not_the_agent_id() {
        let pinned = seed_for_command("env FOO=1 npx @agentclientprotocol/claude-agent-acp@0.70.0")
            .expect("a version pin and an env prefix still resolve");
        assert_eq!(ids(&pinned.modes), ids(&claude_seed().modes));
    }

    /// A name the agent advertised wins; one it did not advertise is still
    /// the seed's; an id nothing names is shown as itself.
    #[test]
    fn a_mode_is_named_from_what_was_advertised_then_the_seed() {
        let claude = "npx -y @agentclientprotocol/claude-agent-acp@latest";
        let advertised = [VocabEntry::new("plan", "Planning")];
        assert_eq!(
            mode_display_name(Some(&advertised), claude, "plan"),
            "Planning"
        );
        assert_eq!(
            mode_display_name(Some(&[]), claude, "bypassPermissions"),
            "Bypass Permissions"
        );
        assert_eq!(mode_display_name(None, "other --acp", "yolo"), "yolo");
    }
}
