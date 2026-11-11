//! Public API tests for remote.

use daruda_config::remote::*;

#[test]
fn legacy_config_leaves_new_channels_inert() {
    let cfg: daruda_config::Config =
        toml::from_str("[telegram]\nenabled = true\nauthorized_chat_id = 42\n").unwrap();
    assert!(cfg.telegram.enabled);
    assert!(cfg.remote.channels.is_empty());
}

#[test]
fn round_trip_preserves_opaque_ids_and_explicit_presence_setting() {
    let source = "[[channels]]\nid = 'work'\nkind = 'slack'\nenabled = true\nonly_when_away = false\n[channels.recipient]\nuser_id = 'U1'\nconversation_id = 'D1'\nscope_id = 'T1'\n";
    let cfg: RemoteConfig = toml::from_str(source).unwrap();
    assert!(cfg.validate().is_ok());
    assert!(!cfg.channels[0].only_when_away);
    assert_eq!(
        toml::from_str::<RemoteConfig>(&toml::to_string(&cfg).unwrap()).unwrap(),
        cfg
    );
    let mut wrong = cfg.channels[0].recipient.clone().unwrap();
    wrong.user_id = "U2".into();
    assert!(!cfg.channels[0].accepts(&wrong));
    wrong = cfg.channels[0].recipient.clone().unwrap();
    wrong.scope_id = "T2".into();
    assert!(!cfg.channels[0].accepts(&wrong));
}

#[test]
fn duplicate_ids_and_invalid_credential_keys_are_rejected() {
    let channel = ChannelConfig::new("work".into(), ChannelKind::Slack);
    assert!(
        RemoteConfig {
            channels: vec![channel.clone(), channel]
        }
        .validate()
        .is_err()
    );
    let invalid = ChannelConfig::new("../token".into(), ChannelKind::Discord);
    assert!(
        RemoteConfig {
            channels: vec![invalid]
        }
        .validate()
        .is_err()
    );
}

/// Opening the app's page names a person but no conversation: only the
/// paired person on an enabled channel is accepted, in any conversation.
#[test]
fn a_person_is_accepted_by_user_and_scope_alone() {
    let mut channel = ChannelConfig::new("work".into(), ChannelKind::Slack);
    channel.recipient = Some(RemoteRecipient {
        user_id: "U1".into(),
        conversation_id: "D1".into(),
        scope_id: "T1".into(),
        conversation_kind: ConversationKind::Unknown,
    });
    assert!(!channel.accepts_person("U1", "T1"), "a disabled channel");
    channel.enabled = true;
    let cases = [
        ("U1", "T1", true, "the paired person"),
        ("U2", "T1", false, "another member of the team"),
        ("U1", "T2", false, "the same id in another team"),
    ];
    for (user, scope, want, label) in cases {
        assert_eq!(channel.accepts_person(user, scope), want, "{label}");
    }
    channel.recipient = None;
    assert!(!channel.accepts_person("U1", "T1"), "an unpaired channel");
}
