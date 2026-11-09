//! How a Telegram update is spelled in the remote-channel trace: its shape,
//! chat and payload, for the poll loop's `inbound` line.

use crate::remote_channel::trace;
use crate::telegram::client::UpdateKind;

/// A stable short name per inbound update shape, for the `inbound` line.
pub(crate) fn kind_name(kind: &UpdateKind) -> &'static str {
    match kind {
        UpdateKind::Message { .. } => "message",
        UpdateKind::Callback { .. } => "callback",
        UpdateKind::Unsupported => "unsupported",
    }
}

/// The chat an inbound update came from, when its shape names one.
pub(crate) fn chat_id(kind: &UpdateKind) -> Option<i64> {
    match kind {
        UpdateKind::Message { chat_id, .. } | UpdateKind::Callback { chat_id, .. } => {
            Some(*chat_id)
        }
        UpdateKind::Unsupported => None,
    }
}

/// What an inbound update carries: a message's body, or a callback's payload
/// token. Previewed, so a long prompt does not take the line over.
pub(crate) fn payload(kind: &UpdateKind) -> String {
    match kind {
        UpdateKind::Message { text, .. } => trace::preview(text),
        UpdateKind::Callback { data, .. } => trace::preview(data),
        UpdateKind::Unsupported => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inbound_message_traces_its_chat_and_body() {
        let kind = UpdateKind::Message {
            chat_id: -1001,
            text: "ship it".to_string(),
            reply_to_message_id: None,
        };
        assert_eq!(kind_name(&kind), "message");
        assert_eq!(chat_id(&kind), Some(-1001));
        assert_eq!(payload(&kind), "ship it");
    }

    #[test]
    fn an_inbound_callback_traces_its_opaque_token() {
        let kind = UpdateKind::Callback {
            chat_id: 42,
            callback_id: "cb".to_string(),
            data: "perm:9:allow".to_string(),
            message_id: 7,
            message_text: "Allow?".to_string(),
        };
        assert_eq!(kind_name(&kind), "callback");
        assert_eq!(chat_id(&kind), Some(42));
        assert_eq!(payload(&kind), "perm:9:allow");
    }

    #[test]
    fn an_unsupported_update_names_no_chat_and_carries_no_payload() {
        let kind = UpdateKind::Unsupported;
        assert_eq!(kind_name(&kind), "unsupported");
        assert_eq!(chat_id(&kind), None);
        assert_eq!(payload(&kind), "");
    }
}
