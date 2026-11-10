use super::{
    VocabEntry, agent_row_delegated_options, agent_row_summary, agent_row_vocabulary_options,
    vocabulary_options,
};
use daruda_store::agent_vocabulary::AgentVocabularyCache;

fn entries(pairs: &[(&str, &str)]) -> Vec<VocabEntry> {
    pairs
        .iter()
        .map(|(id, name)| VocabEntry::new(*id, *name))
        .collect()
}

const CLAUDE: &str = "npx -y @agentclientprotocol/claude-agent-acp@latest";

fn values(options: &[super::SelectOption]) -> Vec<String> {
    options.iter().map(|o| o.value.to_string()).collect()
}

/// The empty sentinel leads the list and is what `collect_agent_catalog`
/// maps back to "no override", so it must never be a real mode id.
#[test]
fn the_agent_default_entry_comes_first_and_carries_an_empty_value() {
    let options = vocabulary_options(
        &entries(&[("plan", "Plan")]),
        Some("plan"),
        "",
        super::MODE_LABELS,
    );
    assert_eq!(values(&options), vec!["", "plan"]);
    assert!(
        options[0].label.contains("Plan"),
        "the entry names the adapter's own default: {}",
        options[0].label
    );
}

#[test]
fn an_unknown_adapter_default_leaves_the_entry_unnamed() {
    let unlisted = vocabulary_options(&entries(&[("plan", "Plan")]), None, "", super::MODE_LABELS);
    let named = vocabulary_options(
        &entries(&[("plan", "Plan")]),
        Some("plan"),
        "",
        super::MODE_LABELS,
    );
    assert_ne!(unlisted[0].label, named[0].label);
    assert_eq!(values(&unlisted), vec!["", "plan"]);
}

/// The delegated picker's "no override" entry names the policy default it
/// stands for, not the adapter's own default, and falls back to the
/// session mode for an adapter with no such policy.
#[test]
fn the_delegated_default_entry_names_the_policy_mode() {
    let vocabulary = AgentVocabularyCache::default();
    let claude = agent_row_delegated_options(&vocabulary, "claude", CLAUDE, "");
    assert_eq!(
        claude[0].label.as_ref(),
        crate::strings::settings::agent_delegated_mode_default_named("Bypass Permissions")
    );
    assert!(values(&claude).contains(&"plan".to_string()));
    let codex = agent_row_delegated_options(&vocabulary, "codex", "npx codex-acp", "");
    assert_eq!(
        codex[0].label.as_ref(),
        crate::strings::settings::agent_delegated_mode_default_named("Agent (full access)")
    );
    let other = agent_row_delegated_options(&vocabulary, "other", "other --acp", "legacy");
    assert_eq!(
        other[0].label.as_ref(),
        crate::strings::settings::agent_delegated_mode_default_named(
            crate::strings::settings::agent_field_default_mode()
        )
    );
    assert_eq!(values(&other), vec!["", "legacy"]);
}

const FOLLOWS: &str = super::DELEGATED_FOLLOWS_SESSION;

/// While a blank is stored, "Same as session mode" sits right after the
/// policy default on every adapter, and the blank selects it rather than
/// the default or an appended blank entry.
#[test]
fn the_delegated_picker_offers_same_as_session_mode_for_a_blank_value() {
    let vocabulary = AgentVocabularyCache::default();
    for command in [CLAUDE, "npx codex-acp", "other --acp"] {
        let options = agent_row_delegated_options(&vocabulary, "a", command, FOLLOWS);
        assert_eq!(options[1].value.as_ref(), FOLLOWS, "{command}");
        assert_eq!(
            options[1].label.as_ref(),
            crate::strings::settings::agent_delegated_mode_follows_session_mode()
        );
        assert_eq!(
            values(&options).iter().filter(|v| *v == FOLLOWS).count(),
            1,
            "{command}"
        );
    }
    assert_eq!(super::delegated_picker_value(None).as_ref(), "");
    for blank in ["", "  "] {
        assert_eq!(super::delegated_picker_value(Some(blank)).as_ref(), FOLLOWS);
    }
    assert_eq!(super::delegated_picker_value(Some("plan")).as_ref(), "plan");
}

/// An adapter with no policy default follows the session mode from its
/// Default entry, so a second entry doing the same is not offered; one
/// with a policy default offers it as the way out of that default.
#[test]
fn same_as_session_mode_is_offered_only_where_it_differs_from_default() {
    let vocabulary = AgentVocabularyCache::default();
    for (command, offered) in [
        (CLAUDE, true),
        ("npx codex-acp", true),
        ("other --acp", false),
    ] {
        for saved in ["", "plan"] {
            let options = agent_row_delegated_options(&vocabulary, "a", command, saved);
            assert_eq!(
                values(&options).iter().any(|v| v == FOLLOWS),
                offered,
                "{command} {saved:?}"
            );
        }
    }
}

/// A value pinned before the agent ever advertised anything has to stay
/// selectable, or opening Settings would drop it on the next save.
#[test]
fn a_saved_value_outside_the_vocabulary_is_appended() {
    let options = vocabulary_options(
        &entries(&[("plan", "Plan")]),
        None,
        "legacy",
        super::MODE_LABELS,
    );
    assert_eq!(values(&options), vec!["", "plan", "legacy"]);
}

#[test]
fn a_saved_value_already_in_the_vocabulary_is_not_duplicated() {
    let options = vocabulary_options(
        &entries(&[("plan", "Plan")]),
        None,
        "plan",
        super::MODE_LABELS,
    );
    assert_eq!(values(&options), vec!["", "plan"]);
}

#[test]
fn an_empty_vocabulary_still_offers_the_agent_default() {
    assert_eq!(
        values(&vocabulary_options(&[], None, "", super::MODE_LABELS)),
        vec![""]
    );
}

/// The default model names what it resolves to, the same label the
/// agent-chat chip shows.
#[test]
fn the_default_model_names_the_model_it_resolves_to() {
    let mut cache = AgentVocabularyCache::default();
    cache.record_models(
        "claude",
        CLAUDE,
        vec![
            VocabEntry::new("default", "Default (recommended)")
                .with_description(Some("Opus 5.5".to_string())),
            VocabEntry::new("opus", "Opus 5.5")
                .with_description(Some("For complex work".to_string())),
        ],
    );

    let (_modes, models) = agent_row_vocabulary_options(&cache, "claude", CLAUDE, "", "");

    let labels: Vec<String> = models.iter().map(|o| o.label.to_string()).collect();
    assert_eq!(labels[1..], ["Default (Opus 5.5)", "Opus 5.5"]);
    assert_eq!(
        labels[0],
        crate::strings::settings::agent_vocabulary_agent_default_named("Opus 5.5"),
        "the agent-default entry names the model, not the default choice"
    );
}

/// A collapsed card names the picked value, or — picking nothing — what
/// the adapter itself defaults to, from the same vocabulary the pickers use.
#[test]
fn the_summary_names_the_pick_or_the_adapter_default() {
    let command = "npx -y @agentclientprotocol/claude-agent-acp@latest";
    let mut cache = AgentVocabularyCache::default();
    cache.record_models(
        "claude",
        command,
        entries(&[("default", "Default (recommended)"), ("opus", "Opus 5.5")]),
    );
    cache.record_modes(
        "claude",
        command,
        entries(&[("default", "Manual"), ("plan", "Plan")]),
    );

    assert_eq!(
        agent_row_summary(&cache, "claude", command, "", ""),
        ("Default (recommended)".to_string(), "Manual".to_string())
    );
    // The model axis names it the way the chat's model chip does.
    cache.record_models(
        "claude",
        command,
        vec![
            VocabEntry::new("default", "Default (recommended)")
                .with_description(Some("Opus 5.5".to_string())),
            VocabEntry::new("opus", "Opus 5.5")
                .with_description(Some("For complex work".to_string())),
        ],
    );
    assert_eq!(
        agent_row_summary(&cache, "claude", command, "", "").0,
        "Default (Opus 5.5)"
    );
    assert_eq!(
        agent_row_summary(&cache, "claude", command, "plan", "opus"),
        ("Opus 5.5".to_string(), "Plan".to_string())
    );
    assert_eq!(
        agent_row_summary(&cache, "claude", command, "plan", "default").0,
        "Default (Opus 5.5)"
    );
    // A pick the vocabulary does not list still names itself.
    assert_eq!(
        agent_row_summary(&cache, "claude", command, "legacy", "").1,
        "legacy"
    );
}

#[test]
fn a_known_empty_axis_does_not_fall_back_to_the_seed() {
    let mut cache = AgentVocabularyCache::default();
    cache.record_models(
        "claude",
        "npx -y @agentclientprotocol/claude-agent-acp@latest",
        Vec::new(),
    );

    let (_modes, models) = agent_row_vocabulary_options(
        &cache,
        "claude",
        "npx -y @agentclientprotocol/claude-agent-acp@latest",
        "",
        "",
    );

    assert_eq!(
        values(&models),
        vec![""],
        "the agent connected and advertised no models, so the Claude seed must stay suppressed"
    );
}
