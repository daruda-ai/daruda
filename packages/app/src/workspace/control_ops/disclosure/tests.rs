use crate::test_support::workspace_with_agent_chat;
use daruda_control_types::agent_text::isolate;

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
        assert_eq!(disclosure.agent_name, isolate(&claude.name));
        assert_eq!(disclosure.mode, isolate("Bypass Permissions"));
        // The same agent opened for a person asks for its ordinary mode.
        assert_ne!(
            ws.control_chat_disclosure(&id, &daruda_config::ModeChoice::FollowDefault)
                .mode,
            isolate("Bypass Permissions")
        );
        let source = claude.launch.adapter_command().to_owned();
        ws.agent_vocabulary.record_modes(&id, &source, Vec::new());
        assert_eq!(
            ws.control_chat_disclosure(&id, &delegated).mode,
            isolate("Bypass Permissions")
        );
        assert_eq!(
            ws.control_chat_disclosure("custom", &delegated).mode,
            isolate("yolo")
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
        assert_eq!(
            disclosure.agent_name,
            format!("{}{hidden}", isolate("Claude Code"))
        );
        assert_eq!(disclosure.mode, isolate("Bypass Permissions"));
        ws.agent_vocabulary
            .record_modes(&id, &source, vec![entry("Ask\u{202E}\nfirst")]);
        assert_eq!(
            ws.control_chat_disclosure(&id, &delegated).mode,
            crate::surface::strings::control::approval_mode_adapter_named(
                format!("{}{hidden}", isolate("Ask first")),
                isolate("bypassPermissions")
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
            isolate(&c.repeat(HEAD)),
            s::approval_prompt_elided(n - HEAD - TAIL),
            isolate(&c.repeat(TAIL))
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
