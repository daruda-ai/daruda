//! Slack-only placement of a pane's messages: a root that names the pane, and
//! replies under it. Other transports have no thread concept here; callers
//! ask [`crate::remote_channel::transport::pane_threads`] before using this.

use super::{API, Credentials, Message, Result, SentPart, api, post, post_payload, update_result};
use crate::remote_channel::cards::EditFailure;
use crate::remote_channel::transport::http;
use daruda_config::remote::RemoteRecipient;
use serde_json::{Value, json};

/// What a root shows: a status line, when one is known, over the pane's name.
pub(crate) struct RootView<'a> {
    pub title: &'a str,
    /// Slack emoji name and the status words.
    pub status: Option<(&'a str, &'a str)>,
}

impl RootView<'_> {
    fn blocks(&self) -> Vec<Value> {
        let mut elements = Vec::new();
        if let Some((emoji, label)) = self.status {
            elements.push(json!({"type": "emoji", "name": emoji}));
            elements.push(
                json!({"type": "text", "text": format!(" {label}\n"), "style": {"bold": true}}),
            );
        }
        elements.push(json!({"type": "text", "text": self.title}));
        vec![json!({"type": "rich_text", "elements": [
            {"type": "rich_text_section", "elements": elements}]})]
    }

    /// The notification and accessibility text.
    fn fallback(&self) -> String {
        match self.status {
            Some((_, label)) => format!("{label}\n{}", self.title),
            None => self.title.to_owned(),
        }
    }
}

/// Post the thread root naming the pane. Its `ts` is the thread's id.
pub(crate) fn post_root(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    view: &RootView<'_>,
) -> Result<String> {
    let response = api(
        &credentials.bot,
        "chat.postMessage",
        &root_payload(recipient, view),
    )?;
    Ok(http::required(&response, "ts")?.to_owned())
}

fn root_payload(recipient: &RemoteRecipient, view: &RootView<'_>) -> Value {
    post_payload(recipient, &view.fallback(), view.blocks(), None)
}

/// Show `view` on the root `root` in place.
pub(crate) fn replace_root(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    root: &str,
    view: &RootView<'_>,
) -> std::result::Result<(), EditFailure> {
    update_result(http::request_with_status(
        "POST",
        &format!("{API}/chat.update"),
        Some(&format!("Bearer {}", credentials.bot)),
        &json!({"channel": recipient.conversation_id, "ts": root, "text": view.fallback(),
            "parse": "none", "blocks": view.blocks()}),
    ))
}

/// Post `message` as replies under `root`.
pub(crate) fn post_in_thread(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    root: &str,
    message: Message,
    on_sent: impl FnMut(SentPart),
) -> Result<Vec<String>> {
    post(credentials, recipient, message, Some(root), on_sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient() -> RemoteRecipient {
        RemoteRecipient {
            user_id: "U1".into(),
            conversation_id: "D1".into(),
            scope_id: "T1".into(),
        }
    }

    #[test]
    fn a_root_shows_its_status_over_the_literal_pane_name() {
        let titled = RootView {
            title: "my_project\nClaude",
            status: None,
        };
        let payload = root_payload(&recipient(), &titled);
        assert_eq!(payload["channel"], "D1");
        assert_eq!(payload["text"], "my_project\nClaude");
        assert!(payload.get("thread_ts").is_none());
        let elements = &payload["blocks"][0]["elements"][0]["elements"];
        assert_eq!(elements.as_array().unwrap().len(), 1, "no status yet");
        assert_eq!(
            elements[0],
            json!({"type": "text", "text": "my_project\nClaude"})
        );

        let working = RootView {
            title: "my_project\nClaude",
            status: Some(("hourglass_flowing_sand", "Working since 2:05 PM")),
        };
        let payload = root_payload(&recipient(), &working);
        assert_eq!(payload["text"], "Working since 2:05 PM\nmy_project\nClaude");
        let elements = &payload["blocks"][0]["elements"][0]["elements"];
        assert_eq!(
            elements[0],
            json!({"type": "emoji", "name": "hourglass_flowing_sand"})
        );
        assert_eq!(elements[1]["text"], " Working since 2:05 PM\n");
        assert_eq!(elements[2]["text"], "my_project\nClaude");
    }

    #[test]
    fn a_reply_carries_its_root() {
        let payload = post_payload(&recipient(), "done", Vec::new(), Some("1.2"));
        assert_eq!(payload["thread_ts"], "1.2");
        assert!(
            post_payload(&recipient(), "done", Vec::new(), None)
                .get("thread_ts")
                .is_none()
        );
    }
}
