//! Slack-only placement of a pane's messages: a root that names the pane, and
//! replies under it. Other transports have no thread concept here; callers
//! ask [`crate::remote_channel::transport::pane_threads`] before using this.

use super::{Credentials, Message, Result, SentPart, api, literal_block, post, post_payload};
use crate::remote_channel::transport::http;
use daruda_config::remote::RemoteRecipient;

/// Post the thread root naming the pane. Its `ts` is the thread's id.
pub(crate) fn post_root(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    title: &str,
) -> Result<String> {
    let response = api(
        &credentials.bot,
        "chat.postMessage",
        &root_payload(recipient, title),
    )?;
    Ok(http::required(&response, "ts")?.to_owned())
}

fn root_payload(recipient: &RemoteRecipient, title: &str) -> serde_json::Value {
    post_payload(recipient, title, vec![literal_block(title)], None)
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
    fn a_root_names_the_pane_literally_and_starts_no_thread() {
        let payload = root_payload(&recipient(), "my_project\nClaude");
        assert_eq!(payload["channel"], "D1");
        assert_eq!(payload["text"], "my_project\nClaude");
        assert!(payload.get("thread_ts").is_none());
        assert_eq!(payload["blocks"][0]["type"], "rich_text");
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
