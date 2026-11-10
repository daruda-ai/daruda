//! Slack-only placement of a pane's messages: a root that names the pane, and
//! replies under it. Other transports have no thread concept here; callers
//! ask [`crate::remote_channel::transport::pane_threads`] before using this.

use super::{
    API, Credentials, Message, Result, SentPart, TRANSIENT_REFUSALS, api, post, post_payload,
    refusal,
};
use crate::remote_channel::transport::http;
use daruda_config::remote::RemoteRecipient;
use serde_json::{Value, json};

/// What a root shows: a status line, when one is known, over the pane's name.
pub(crate) struct RootView<'a> {
    pub title: &'a str,
    /// Slack emoji name and the status words.
    pub status: Option<(&'a str, &'a str)>,
    /// The stop button's label and value, while there is work to stop.
    pub stop: Option<(&'a str, &'a str)>,
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
        let mut blocks = vec![json!({"type": "rich_text", "elements": [
            {"type": "rich_text_section", "elements": elements}]})];
        if let Some((label, value)) = self.stop {
            blocks.push(json!({"type": "actions", "elements": [{
                "type": "button", "action_id": "thread-stop", "style": "danger",
                "text": {"type": "plain_text", "text": label}, "value": value,
            }]}));
        }
        blocks
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

/// Why a root could not be updated.
#[derive(Debug)]
pub(crate) enum RootFailure {
    /// The message or its conversation no longer exists.
    Missing(String),
    /// Slack refused this update: credentials, scopes, or the payload.
    Refused(String),
    /// Rate limits, timeouts, server trouble: may pass.
    Transient(String),
}

/// Codes that mean the root itself is gone, as opposed to this edit being
/// refused. Only these let a thread's record go.
const MISSING: &[&str] = &["message_not_found", "channel_not_found"];

/// Show `view` on the root `root` in place.
pub(crate) fn replace_root(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    root: &str,
    view: &RootView<'_>,
) -> std::result::Result<(), RootFailure> {
    root_result(http::request_with_status(
        "POST",
        &format!("{API}/chat.update"),
        Some(&format!("Bearer {}", credentials.bot)),
        &json!({"channel": recipient.conversation_id, "ts": root, "text": view.fallback(),
            "parse": "none", "blocks": view.blocks()}),
    ))
}

fn root_result(
    response: std::result::Result<Value, http::Failure>,
) -> std::result::Result<(), RootFailure> {
    let value = match response {
        Ok(value) => value,
        Err(failure) => {
            let retryable = matches!(&failure, http::Failure::Transport(_))
                || matches!(&failure, http::Failure::Status(status) if http::retryable_status(*status));
            let reason =
                crate::remote_channel::transport::TransportError::from(failure).to_string();
            return Err(if retryable {
                RootFailure::Transient(reason)
            } else {
                RootFailure::Refused(reason)
            });
        }
    };
    let Some(code) = refusal(&value) else {
        return Ok(());
    };
    let reason = format!("Slack chat.update failed: {code}");
    Err(
        if !value["error"].is_string() || TRANSIENT_REFUSALS.contains(&code.as_str()) {
            RootFailure::Transient(reason)
        } else if MISSING.contains(&code.as_str()) {
            RootFailure::Missing(reason)
        } else {
            RootFailure::Refused(reason)
        },
    )
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
            stop: None,
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
            stop: Some(("Stop", "thr:abc")),
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
        let button = &payload["blocks"][1]["elements"][0];
        assert_eq!(button["value"], "thr:abc");
        assert_eq!(button["text"]["text"], "Stop");
        assert_eq!(
            root_payload(&recipient(), &titled)["blocks"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "no button without work to stop"
        );
    }

    /// Only a missing message lets a thread go; a refusal of the edit keeps
    /// the thread routable, and passing trouble is retried.
    #[test]
    fn a_root_update_failure_is_classified_by_what_it_says_of_the_root() {
        use super::super::http::Failure;
        let error = |code: &str| Ok(json!({"ok": false, "error": code}));
        let cases = [
            (Ok(json!({"ok": true})), "ok"),
            (error("message_not_found"), "missing"),
            (error("channel_not_found"), "missing"),
            (error("invalid_auth"), "refused"),
            (error("missing_scope"), "refused"),
            (error("invalid_blocks"), "refused"),
            (error("ratelimited"), "transient"),
            (Ok(json!({"ok": false})), "transient"),
            (Err(Failure::Status(503)), "transient"),
            (Err(Failure::Status(403)), "refused"),
        ];
        for (response, want) in cases {
            let shown = format!("{response:?}");
            let got = match root_result(response) {
                Ok(()) => "ok",
                Err(RootFailure::Missing(_)) => "missing",
                Err(RootFailure::Refused(_)) => "refused",
                Err(RootFailure::Transient(_)) => "transient",
            };
            assert_eq!(got, want, "{shown}");
        }
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
