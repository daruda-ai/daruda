use super::*;

#[test]
fn file_input_tokens_select_the_requested_interaction_state() {
    use crate::workspace::main_area::file_context::FileInputShot;
    for (name, state) in [
        ("file-input-selection", FileInputShot::Selection),
        ("file-input-menu", FileInputShot::Menu),
        ("file-input-inserted", FileInputShot::Inserted),
    ] {
        assert_eq!(
            ScreenshotScenario::from_cli_name(name),
            Some(ScreenshotScenario::FileInput(state)),
            "{name}"
        );
    }
}

#[test]
fn unknown_name_maps_to_none() {
    assert_eq!(ScreenshotScenario::from_cli_name("nope"), None);
    assert_eq!(ScreenshotScenario::from_cli_name(""), None);
}

#[test]
fn bare_settings_maps_to_default_section() {
    assert_eq!(
        ScreenshotScenario::from_cli_name("settings"),
        Some(ScreenshotScenario::Settings(BuiltinSection::default()))
    );
}

#[test]
fn settings_slug_maps_to_that_section() {
    assert_eq!(
        ScreenshotScenario::from_cli_name("settings:font"),
        Some(ScreenshotScenario::Settings(BuiltinSection::Font))
    );
    assert_eq!(
        ScreenshotScenario::from_cli_name("settings:notifications"),
        Some(ScreenshotScenario::Settings(BuiltinSection::Notifications))
    );
}

#[test]
fn the_settings_error_token_is_its_own_scenario() {
    assert_eq!(
        ScreenshotScenario::from_cli_name("settings-error"),
        Some(ScreenshotScenario::SettingsError),
        "the hyphenated token must not be read as a `settings:<slug>` section"
    );
    assert_eq!(
        ScreenshotScenario::from_cli_name("settings-search"),
        Some(ScreenshotScenario::SettingsSearch)
    );
    assert_eq!(
        ScreenshotScenario::from_cli_name("agent-catalog-expanded"),
        Some(ScreenshotScenario::AgentCatalogExpanded)
    );
}

#[test]
fn unknown_settings_slug_maps_to_none() {
    assert_eq!(ScreenshotScenario::from_cli_name("settings:bogus"), None);
}

#[test]
fn every_thoughts_shot_is_addressable() {
    for (name, shot) in [
        ("agent-chat-thoughts", ThoughtsShot::Settled),
        ("agent-chat-thoughts-working", ThoughtsShot::Working),
        ("agent-chat-thoughts-only", ThoughtsShot::ThoughtsOnly),
    ] {
        assert_eq!(
            ScreenshotScenario::from_cli_name(name),
            Some(ScreenshotScenario::AgentChatThoughts(shot))
        );
    }
}

#[test]
fn every_question_shot_is_addressable() {
    for (name, shot) in [
        ("agent-chat-question", QuestionShot::Single),
        ("agent-chat-question-form", QuestionShot::Form),
        (
            "agent-chat-question-long",
            QuestionShot::LongMessage { expanded: false },
        ),
        (
            "agent-chat-question-long-expanded",
            QuestionShot::LongMessage { expanded: true },
        ),
        ("agent-chat-question-answered", QuestionShot::Answered),
        ("agent-chat-question-folded", QuestionShot::Folded),
        ("agent-chat-question-composer", QuestionShot::Composer),
        (
            "agent-chat-question-blocked-prompt",
            QuestionShot::BlockedPrompt,
        ),
        ("agent-chat-question-countdown", QuestionShot::Countdown),
    ] {
        assert_eq!(
            ScreenshotScenario::from_cli_name(name),
            Some(ScreenshotScenario::AgentChatQuestion(shot))
        );
    }
}

/// Every tab the panel offers has to be reachable from the CLI, or a tab
/// silently loses its permanent capture coverage.
#[test]
fn every_options_tab_is_addressable_by_its_own_token() {
    assert_eq!(
        ScreenshotScenario::from_cli_name("agent-chat-options"),
        Some(ScreenshotScenario::AgentChatOptions(
            ActivityOptionsTab::Fold
        ))
    );
    for tab in ActivityOptionsTab::ALL {
        assert_eq!(
            ScreenshotScenario::from_cli_name(&format!("agent-chat-options:{}", tab.token())),
            Some(ScreenshotScenario::AgentChatOptions(tab)),
            "{tab:?}"
        );
    }
    assert_eq!(
        ScreenshotScenario::from_cli_name("agent-chat-options:nope"),
        None
    );
}

/// Each Settings editor is reachable open, or its panel loses capture
/// coverage — a closed field shows only the trigger.
#[test]
fn every_catalog_editor_is_addressable_by_its_own_token() {
    use daruda_settings::EditorShot;
    assert_eq!(
        ScreenshotScenario::from_cli_name("agent-catalog-editor"),
        Some(ScreenshotScenario::AgentCatalogEditor(EditorShot::Fold))
    );
    for axis in EditorShot::ALL {
        assert_eq!(
            ScreenshotScenario::from_cli_name(&format!("agent-catalog-editor:{}", axis.token())),
            Some(ScreenshotScenario::AgentCatalogEditor(axis)),
            "{axis:?}"
        );
    }
    assert_eq!(
        ScreenshotScenario::from_cli_name("agent-catalog-editor:nope"),
        None
    );
}

#[test]
fn link_scenarios_have_distinct_cli_names() {
    assert_eq!(
        ScreenshotScenario::from_cli_name("agent-chat-theme"),
        Some(ScreenshotScenario::AgentChatTheme)
    );
    let links = ScreenshotScenario::from_cli_name("agent-chat-links");
    let menu = ScreenshotScenario::from_cli_name("agent-chat-link-menu");
    assert!(links.is_some());
    assert!(menu.is_some());
    assert_ne!(links, menu);
}
