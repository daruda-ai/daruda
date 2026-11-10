//! Non-secret, per-connection settings for remote control channels.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Slack,
    Discord,
}

impl ChannelKind {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Discord => "discord",
        }
    }
}

/// All three coordinates must match before an inbound event can control the app.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RemoteRecipient {
    pub user_id: String,
    pub conversation_id: String,
    /// Slack team or Discord guild. Empty for a Discord direct message.
    pub scope_id: String,
    /// Who else reads what daruda posts there. Recorded at pairing; not one
    /// of the coordinates an inbound event must match.
    #[serde(default, skip_serializing_if = "ConversationKind::is_unknown")]
    pub conversation_kind: ConversationKind,
}

impl RemoteRecipient {
    /// Whether `other` names the same person in the same conversation.
    pub fn same_party(&self, other: &RemoteRecipient) -> bool {
        self.user_id == other.user_id
            && self.conversation_id == other.conversation_id
            && self.scope_id == other.scope_id
    }
}

/// Who besides the person reads a conversation daruda posts to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    /// A direct message with the bot: only the person.
    Direct,
    /// A channel or group conversation: its other members too.
    Shared,
    /// Paired before daruda recorded which.
    #[default]
    Unknown,
}

impl ConversationKind {
    pub fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }

    /// The serialized name; `None` for the default, which is left unwritten.
    pub const fn slug(self) -> Option<&'static str> {
        match self {
            Self::Direct => Some("direct"),
            Self::Shared => Some("shared"),
            Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChannelConfig {
    pub id: String,
    pub kind: ChannelKind,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_only_when_away")]
    pub only_when_away: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<RemoteRecipient>,
}

fn default_only_when_away() -> bool {
    true
}

impl ChannelConfig {
    pub fn new(id: String, kind: ChannelKind) -> Self {
        Self {
            id,
            kind,
            enabled: false,
            only_when_away: true,
            recipient: None,
        }
    }

    pub fn accepts(&self, sender: &RemoteRecipient) -> bool {
        self.enabled
            && self
                .recipient
                .as_ref()
                .is_some_and(|recipient| recipient.same_party(sender))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct RemoteConfig {
    pub channels: Vec<ChannelConfig>,
}

impl RemoteConfig {
    /// Duplicate IDs are rejected so two connections cannot share credentials.
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for channel in &self.channels {
            if channel.id.is_empty()
                || channel.id.len() > 64
                || !channel
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
            {
                return Err(
                    "Remote channel IDs must contain 1-64 ASCII letters, digits, '-' or '_'".into(),
                );
            }
            if !ids.insert(&channel.id) {
                return Err(format!("Duplicate remote channel ID: {}", channel.id));
            }
            if channel.recipient.as_ref().is_some_and(|r| {
                r.user_id.is_empty()
                    || r.conversation_id.is_empty()
                    || (channel.kind == ChannelKind::Slack && r.scope_id.is_empty())
            }) {
                return Err(format!("Incomplete remote recipient: {}", channel.id));
            }
        }
        Ok(())
    }
}

pub(crate) fn patch_document(doc: &mut toml_edit::DocumentMut, config: &RemoteConfig) {
    crate::patch_section(doc, "remote", |remote| {
        let mut channels = toml_edit::ArrayOfTables::new();
        for channel in &config.channels {
            let mut table = toml_edit::Table::new();
            table["id"] = toml_edit::value(channel.id.clone());
            table["kind"] = toml_edit::value(channel.kind.slug());
            table["enabled"] = toml_edit::value(channel.enabled);
            table["only_when_away"] = toml_edit::value(channel.only_when_away);
            if let Some(recipient) = &channel.recipient {
                let mut target = toml_edit::Table::new();
                target["user_id"] = toml_edit::value(recipient.user_id.clone());
                target["conversation_id"] = toml_edit::value(recipient.conversation_id.clone());
                target["scope_id"] = toml_edit::value(recipient.scope_id.clone());
                if let Some(kind) = recipient.conversation_kind.slug() {
                    target["conversation_kind"] = toml_edit::value(kind);
                }
                table["recipient"] = toml_edit::Item::Table(target);
            }
            channels.push(table);
        }
        remote.insert("channels", toml_edit::Item::ArrayOfTables(channels));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_preserves_other_sections_and_recipient_removal() {
        let mut doc: toml_edit::DocumentMut =
            "# keep\n[telegram]\nenabled = true\nauthorized_chat_id = 42\n[remote]\nextra = 7\n"
                .parse()
                .unwrap();
        let mut config = RemoteConfig {
            channels: vec![ChannelConfig::new("work".into(), ChannelKind::Slack)],
        };
        config.channels[0].recipient = Some(RemoteRecipient {
            user_id: "U1".into(),
            conversation_id: "D1".into(),
            scope_id: "T1".into(),
            conversation_kind: ConversationKind::Shared,
        });
        patch_document(&mut doc, &config);
        let round_trip: crate::Config = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(round_trip.remote, config);
        assert_eq!(round_trip.telegram.authorized_chat_id, Some(42));
        assert!(doc.to_string().contains("extra = 7"));
        config.channels[0].recipient = None;
        patch_document(&mut doc, &config);
        assert!(!doc.to_string().contains("user_id"));
    }

    /// Who else reads a conversation is told to the person, not checked: a
    /// pairing made before daruda recorded it still accepts its sender, and
    /// a tap, which carries no conversation type, is accepted too.
    #[test]
    fn a_sender_is_accepted_whatever_its_conversation_kind() {
        let mut channel = ChannelConfig::new("work".into(), ChannelKind::Slack);
        channel.enabled = true;
        let party = |conversation_kind| RemoteRecipient {
            user_id: "U1".into(),
            conversation_id: "D1".into(),
            scope_id: "T1".into(),
            conversation_kind,
        };
        channel.recipient = Some(party(ConversationKind::Unknown));
        for kind in [
            ConversationKind::Unknown,
            ConversationKind::Direct,
            ConversationKind::Shared,
        ] {
            assert!(channel.accepts(&party(kind)), "{kind:?}");
        }
        let mut stranger = party(ConversationKind::Unknown);
        stranger.user_id = "U2".into();
        assert!(!channel.accepts(&stranger));
    }
}
