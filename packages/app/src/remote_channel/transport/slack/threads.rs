//! Slack-only placement of a pane's messages: a root that names the pane, and
//! replies under it. Other transports have no thread concept here; callers
//! ask [`crate::remote_channel::transport::pane_threads`] before using this.

use super::pane_lines::{PaneLines, StatusLine};
use super::{
    API, Credentials, Message, Reply, Result, SentPart, UpdateVerdict, api, post, post_payload,
    update_verdict,
};
use crate::remote_channel::transport::http;
use daruda_config::remote::RemoteRecipient;
use serde_json::{Value, json};

/// What a root shows: a status line, when one is known, over the pane's name.
pub(crate) struct RootView<'a> {
    pub title: &'a str,
    /// What the pane's conversation is about, under its name.
    pub subject: Option<&'a str>,
    /// Joins name, subject and status in the notification text.
    pub separator: &'a str,
    /// Slack emoji name and the status words.
    pub status: Option<(&'a str, &'a str)>,
    /// The stop button's label and value, while there is work to stop.
    pub stop: Option<(&'a str, &'a str)>,
}

impl RootView<'_> {
    /// The pane's name in bold on its own line, what it is working on below,
    /// then its status, so a thread list tells panes apart before it tells
    /// their states.
    fn blocks(&self) -> Vec<Value> {
        let elements = PaneLines {
            title: self.title,
            subject: self.subject,
            status: self.status.map(|(emoji, label)| StatusLine {
                emoji: Some(emoji),
                label,
                mark: None,
            }),
        }
        .elements();
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

    /// The notification text: the pane first, then its subject and state.
    fn fallback(&self) -> String {
        [
            Some(self.title),
            self.subject,
            self.status.map(|(_, label)| label),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(self.separator)
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
    match update_verdict(response) {
        UpdateVerdict::Updated => Ok(()),
        UpdateVerdict::Transient(reason) => Err(RootFailure::Transient(reason)),
        UpdateVerdict::Refused {
            code: Some(code),
            reason,
        } if MISSING.contains(&code.as_str()) => Err(RootFailure::Missing(reason)),
        UpdateVerdict::Refused { reason, .. } => Err(RootFailure::Refused(reason)),
    }
}

/// Post `message` as replies under `root`; with `broadcast`, also shown in
/// the conversation, where a long-lived thread's root has scrolled away.
pub(crate) fn post_in_thread(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    root: &str,
    broadcast: bool,
    message: Message,
    on_sent: impl FnMut(SentPart),
) -> Result<Vec<String>> {
    let reply = Reply { root, broadcast };
    post(credentials, recipient, message, Some(reply), on_sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient() -> RemoteRecipient {
        RemoteRecipient {
            user_id: "U1".into(),
            conversation_id: "D1".into(),
            scope_id: "T1".into(),
            conversation_kind: daruda_config::remote::ConversationKind::Unknown,
        }
    }

    /// The pane's name leads, in bold, so a thread list and a notification
    /// tell panes apart before they tell their states.
    #[test]
    fn a_root_names_its_pane_first_and_its_status_below() {
        let titled = RootView {
            title: "my_project · Claude",
            subject: None,
            separator: " — ",
            status: None,
            stop: None,
        };
        let payload = root_payload(&recipient(), &titled);
        assert_eq!(payload["channel"], "D1");
        assert_eq!(payload["text"], "my_project · Claude");
        assert!(payload.get("thread_ts").is_none());
        let elements = &payload["blocks"][0]["elements"][0]["elements"];
        assert_eq!(
            elements,
            &json!([{"type": "text", "text": "my_project · Claude", "style": {"bold": true}}]),
            "no status yet"
        );

        let working = RootView {
            title: "my_project · Claude",
            subject: Some("Fix the login bug"),
            separator: " — ",
            status: Some(("hourglass_flowing_sand", "Working since 2:05 PM")),
            stop: Some(("Stop", "thr:abc")),
        };
        let payload = root_payload(&recipient(), &working);
        assert_eq!(
            payload["text"],
            "my_project · Claude — Fix the login bug — Working since 2:05 PM"
        );
        let elements = &payload["blocks"][0]["elements"][0]["elements"];
        assert_eq!(elements[0]["text"], "my_project · Claude");
        assert_eq!(elements[0]["style"]["bold"], true);
        assert_eq!(elements[1]["text"], "\nFix the login bug");
        assert_eq!(elements[2]["text"], "\n");
        assert_eq!(
            elements[3],
            json!({"type": "emoji", "name": "hourglass_flowing_sand"})
        );
        assert_eq!(elements[4]["text"], " Working since 2:05 PM");
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
    fn a_reply_carries_its_root_and_surfaces_only_when_asked() {
        let reply = |broadcast| {
            Some(Reply {
                root: "1.2",
                broadcast,
            })
        };
        let quiet = post_payload(&recipient(), "done", Vec::new(), reply(false));
        assert_eq!(quiet["thread_ts"], "1.2");
        assert!(quiet.get("reply_broadcast").is_none());
        let surfaced = post_payload(&recipient(), "done", Vec::new(), reply(true));
        assert_eq!(surfaced["reply_broadcast"], true);
        let top = post_payload(&recipient(), "done", Vec::new(), None);
        assert!(top.get("thread_ts").is_none() && top.get("reply_broadcast").is_none());
    }
}
