use super::compose_briefing;

#[test]
fn the_recap_leads_the_orchestrator_briefing() {
    assert_eq!(
        compose_briefing(Some("R".into()), Some("B".into())),
        Some("R\n\n---\n\nB".into())
    );
    assert_eq!(compose_briefing(Some("R".into()), None), Some("R".into()));
    assert_eq!(compose_briefing(None, Some("B".into())), Some("B".into()));
    assert_eq!(compose_briefing(None, None), None);
}

use super::{ChatHold, agent_menu_entries};
use crate::workspace::main_area::pane_menu::spec::MenuEntry;

fn agents() -> Vec<(String, String)> {
    vec![
        ("claude".into(), "Claude Code".into()),
        ("codex".into(), "Codex".into()),
        ("gemini".into(), "Gemini".into()),
    ]
}

/// (label, disabled) per item of the submenu at `index`.
fn submenu(entries: &[MenuEntry], index: usize) -> Vec<(String, bool)> {
    let MenuEntry::Submenu { entries, .. } = &entries[index] else {
        panic!("entry {index} is not a submenu");
    };
    entries
        .iter()
        .map(|entry| match entry {
            MenuEntry::Item(spec) => (spec.label().to_string(), spec.is_disabled()),
            _ => panic!("only items expected"),
        })
        .collect()
}

#[test]
fn continue_offers_every_other_agent_and_new_chat_offers_all() {
    let entries = agent_menu_entries(1, &agents(), "claude", ChatHold::Conversation);
    assert_eq!(
        submenu(&entries, 0),
        vec![("Codex".into(), false), ("Gemini".into(), false)]
    );
    assert_eq!(submenu(&entries, 1).len(), 3);
}

#[test]
fn continue_is_disabled_while_the_pane_cannot_hand_off() {
    for hold in [ChatHold::Busy, ChatHold::Pinned] {
        let entries = agent_menu_entries(1, &agents(), "claude", hold);
        assert!(
            submenu(&entries, 0).iter().all(|(_, disabled)| *disabled),
            "{hold:?}"
        );
        assert!(
            submenu(&entries, 1).iter().all(|(_, disabled)| !*disabled),
            "a new chat never touches this pane: {hold:?}"
        );
    }
}
