//! Slack Web API and Socket Mode adapter.

use super::{
    Credentials, HomeEvent, Incoming, IncomingKind, Message, Result, SentPart, TransportError,
    chunks, http, socket,
};
use crate::remote_channel::cards::EditFailure;
use daruda_config::remote::RemoteRecipient;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};

const API: &str = "https://slack.com/api";
/// The one message subtype that is still a reply: a threaded reply the sender
/// also copied to the channel ("Also send to #channel").
const BROADCAST_SUBTYPE: &str = "thread_broadcast";
const SECTION_LIMIT: usize = 3000;
const BUTTON_LABEL_LIMIT: usize = 75;
const BUTTONS_PER_ROW: usize = 25;

fn api(credentials: &str, method: &str, payload: &Value) -> Result<Value> {
    let value = http::request(
        "POST",
        &format!("{API}/{method}"),
        Some(&format!("Bearer {credentials}")),
        payload,
    )?;
    if let Some(code) = refusal(&value) {
        return Err(TransportError::Message(format!(
            "Slack {method} failed: {code}"
        )));
    }
    Ok(value)
}

/// The error code of an `ok: false` reply. Only Slack's symbolic code leaves
/// the adapter, never payload text.
fn refusal(value: &Value) -> Option<String> {
    if value["ok"] == true {
        return None;
    }
    let code = value["error"].as_str().unwrap_or("unknown_error");
    Some(
        code.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(80)
            .collect(),
    )
}

/// `ok: false` codes Slack documents as its own temporary trouble: rate
/// limits, server faults, timeouts and an organization migration in progress.
/// Any other code is a refusal of this request or of the bot's access, which
/// the same request will meet again.
const TRANSIENT_REFUSALS: &[&str] = &[
    "ratelimited",
    "internal_error",
    "fatal_error",
    "service_unavailable",
    "request_timeout",
    "team_added_to_org",
];

pub fn session(
    credentials: &Credentials,
    stop: &AtomicBool,
    mut publish: impl FnMut(Incoming) -> bool,
    mut publish_home: impl FnMut(HomeEvent) -> bool,
    ready: impl FnOnce(),
) -> Result<()> {
    let app = credentials
        .app
        .as_deref()
        .ok_or_else(|| TransportError::Message("Slack app token is missing".into()))?;
    let opened = api(app, "apps.connections.open", &json!({}))?;
    let mut socket = socket::connect(http::required(&opened, "url")?)?;
    ready();
    while !stop.load(Ordering::Relaxed) {
        let Some(envelope) = socket::read(&mut socket)? else {
            continue;
        };
        if envelope["type"] == "disconnect" {
            return Err(TransportError::Message(
                "Slack requested a new connection".into(),
            ));
        }
        if let Some(id) = envelope["envelope_id"].as_str() {
            socket::send(&mut socket, &json!({"envelope_id": id}))?;
        }
        if let Some(home) = home::parse_home(&envelope) {
            if !publish_home(home) {
                break;
            }
            continue;
        }
        if let Some(incoming) = parse(&envelope)
            && !publish(incoming)
        {
            break;
        }
    }
    Ok(())
}

fn parse(envelope: &Value) -> Option<Incoming> {
    let payload = &envelope["payload"];
    match envelope["type"].as_str()? {
        "events_api" => {
            let event = &payload["event"];
            // Subtypes are how Slack marks a message that is *about* a message
            // — an edit, a deletion, a join notice — and none of those are
            // something to relay. `thread_broadcast` is the exception: it is an
            // ordinary threaded reply the sender also copied to the channel,
            // carrying the same `thread_ts` that names the ping it answers. A
            // blanket filter drops it, and the sender hears nothing back.
            let subtype = event["subtype"].as_str();
            if event["type"] != "message"
                || !matches!(subtype, None | Some(BROADCAST_SUBTYPE))
                || event.get("bot_id").is_some()
            {
                return None;
            }
            Some(Incoming {
                event_id: payload["event_id"].as_str()?.to_owned(),
                sender: RemoteRecipient {
                    user_id: event["user"].as_str()?.to_owned(),
                    conversation_id: event["channel"].as_str()?.to_owned(),
                    scope_id: payload["team_id"].as_str()?.to_owned(),
                    conversation_kind: conversation_kind(event["channel_type"].as_str()),
                },
                kind: IncomingKind::Message {
                    text: unescape(event["text"].as_str()?),
                    reply_to: event["thread_ts"].as_str().map(str::to_owned),
                },
            })
        }
        "interactive" if payload["type"] == "block_actions" => Some(Incoming {
            event_id: envelope["envelope_id"].as_str()?.to_owned(),
            sender: RemoteRecipient {
                user_id: payload["user"]["id"].as_str()?.to_owned(),
                conversation_id: payload["channel"]["id"].as_str()?.to_owned(),
                scope_id: payload["team"]["id"].as_str()?.to_owned(),
                conversation_kind: daruda_config::remote::ConversationKind::Unknown,
            },
            kind: IncomingKind::Callback {
                data: payload["actions"][0]["value"].as_str()?.to_owned(),
                message_id: payload["message"]["ts"].as_str()?.to_owned(),
                original: payload["message"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            },
        }),
        _ => None,
    }
}

/// Who else reads a conversation, by Slack's `channel_type`: only a DM with
/// the bot is private to the person; a channel, private channel or group DM
/// has other members.
fn conversation_kind(channel_type: Option<&str>) -> daruda_config::remote::ConversationKind {
    use daruda_config::remote::ConversationKind;
    match channel_type {
        Some("im") => ConversationKind::Direct,
        Some("channel" | "group" | "mpim") => ConversationKind::Shared,
        _ => ConversationKind::Unknown,
    }
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn blocks(
    text: &str,
    keyboard: Option<&crate::remote_channel::bridge::InlineKeyboard>,
    markdown: bool,
) -> Vec<Value> {
    // Slack rejects an empty text object, so a body that rendered to nothing
    // contributes no block at all — the header block still ships.
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(if markdown {
            json!({"type": "section", "text": {"type": "mrkdwn", "text": text, "verbatim": true}})
        } else {
            literal_block(text)
        });
    }
    if let Some(keyboard) = keyboard {
        for (row_index, row) in keyboard.rows.iter().enumerate() {
            for buttons in row.chunks(BUTTONS_PER_ROW) {
                let elements: Vec<_> = buttons
                    .iter()
                    .enumerate()
                    .map(|(index, (label, token))| {
                        let label = chunks(label, BUTTON_LABEL_LIMIT)
                            .into_iter()
                            .next()
                            .unwrap_or_else(|| "?".into());
                        json!({"type": "button", "action_id": format!("remote-{row_index}-{index}"),
                        "text": {"type": "plain_text", "text": label}, "value": token})
                    })
                    .collect();
                blocks.push(json!({"type": "actions", "elements": elements}));
            }
        }
    }
    blocks
}

/// Literal text with its line breaks intact. A `plain_text` section renders
/// every `\n` as a space; rich_text keeps the break and parses no markup, so
/// a file name's `_` stays a `_`.
fn literal_block(text: &str) -> Value {
    json!({"type": "rich_text", "elements": [
        {"type": "rich_text_section", "elements": [{"type": "text", "text": text}]}]})
}

/// One slot per outgoing message. A body that rendered to nothing keeps one
/// empty slot when there is a header to carry, so `chunks`' empty result
/// cannot silently drop the whole message.
fn body_parts(text: &str, header: &str) -> Vec<String> {
    let mut parts = chunks(text, SECTION_LIMIT);
    if parts.is_empty() && !header.is_empty() {
        parts.push(String::new());
    }
    parts
}

pub fn send(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    message: Message,
    on_sent: impl FnMut(SentPart),
) -> Result<Vec<String>> {
    post(credentials, recipient, message, None, on_sent)
}

/// Where a post goes: the top of the conversation, or a reply under a root,
/// also shown in the conversation when `broadcast`.
#[derive(Clone, Copy)]
struct Reply<'a> {
    root: &'a str,
    broadcast: bool,
}

/// Post `message` at the top of the conversation, or as replies under `thread`.
fn post(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    message: Message,
    reply: Option<Reply<'_>>,
    mut on_sent: impl FnMut(SentPart),
) -> Result<Vec<String>> {
    let (text, markdown) = super::format::slack(&message);
    let parts = body_parts(&text, &message.header);
    let mut ids = Vec::new();
    for (index, text) in parts.iter().enumerate() {
        let keyboard = (index + 1 == parts.len())
            .then_some(message.keyboard.as_ref())
            .flatten();
        let mut message_blocks = blocks(text, keyboard, markdown);
        let fallback = if index == 0 && !message.header.is_empty() {
            let header = chunks(&message.header, SECTION_LIMIT)
                .into_iter()
                .next()
                .unwrap_or_default();
            message_blocks.insert(0, literal_block(&header));
            if text.is_empty() {
                header
            } else {
                format!("{header}\n{text}")
            }
        } else {
            text.clone()
        };
        // A broadcast surfaces the message once: a card's last part, which
        // carries its buttons, or else the first, so the conversation shows
        // how the message begins rather than a tail without its start.
        let surfaced = if message.keyboard.is_some() {
            parts.len() - 1
        } else {
            0
        };
        let reply = reply.map(|reply| Reply {
            broadcast: reply.broadcast && index == surfaced,
            ..reply
        });
        let payload = post_payload(recipient, &fallback, message_blocks, reply);
        let response = api(&credentials.bot, "chat.postMessage", &payload)?;
        let id = http::required(&response, "ts")?;
        on_sent(SentPart {
            id: id.to_owned(),
            text: fallback,
            buttons: keyboard.is_some(),
        });
        ids.push(id.to_owned());
    }
    Ok(ids)
}

/// A `chat.postMessage` body: literal fallback text, no unfurling, and a
/// reply under `thread` when given.
fn post_payload(
    recipient: &RemoteRecipient,
    fallback: &str,
    blocks: Vec<Value>,
    reply: Option<Reply<'_>>,
) -> Value {
    let mut payload = json!({
        "channel": recipient.conversation_id, "text": fallback, "mrkdwn": false,
        "parse": "none", "unfurl_links": false, "unfurl_media": false,
        "blocks": blocks,
    });
    if let Some(reply) = reply {
        payload["thread_ts"] = json!(reply.root);
        if reply.broadcast {
            payload["reply_broadcast"] = json!(true);
        }
    }
    payload
}

pub fn edit(
    credentials: &Credentials,
    recipient: &RemoteRecipient,
    message_id: &str,
    original: &str,
    label: &str,
) -> std::result::Result<(), EditFailure> {
    let text = super::feedback(original, label, SECTION_LIMIT);
    update_result(http::request_with_status(
        "POST",
        &format!("{API}/chat.update"),
        Some(&format!("Bearer {}", credentials.bot)),
        &json!({"channel": recipient.conversation_id, "ts": message_id,
        "text": text, "parse": "none", "blocks": blocks(&text, None, false)}),
    ))
}

/// What Slack says of a write that replaces what it shows: `chat.update`
/// for a card's settlement or a thread root's status, `views.publish` for
/// the app home. They differ only in what a refusal means to them. The app
/// home's `chat.getPermalink` read is classified the same way.
enum UpdateVerdict {
    Updated,
    /// May pass: rate limits, server trouble, a reply that names no code.
    Transient(String),
    /// Slack refused this write; `code` when it named one.
    Refused {
        code: Option<String>,
        reason: String,
    },
}

/// Classify a `chat.update` reply.
fn update_verdict(response: std::result::Result<Value, http::Failure>) -> UpdateVerdict {
    verdict("chat.update", response)
}

/// Classify a reply to a write that replaces what Slack shows. Slack refuses
/// with HTTP 200 and an error code; an HTTP error status means the request
/// never reached the method.
fn verdict(method: &str, response: std::result::Result<Value, http::Failure>) -> UpdateVerdict {
    match response {
        Ok(value) => match refusal(&value) {
            None => UpdateVerdict::Updated,
            Some(code) => {
                let reason = format!("Slack {method} failed: {code}");
                // A reply without a code says nothing about the request.
                let named = value["error"].is_string();
                if named && !TRANSIENT_REFUSALS.contains(&code.as_str()) {
                    UpdateVerdict::Refused {
                        code: Some(code),
                        reason,
                    }
                } else {
                    UpdateVerdict::Transient(reason)
                }
            }
        },
        Err(failure) => {
            let retryable = match &failure {
                http::Failure::Status(status) => http::retryable_status(*status),
                http::Failure::Transport(_) => true,
            };
            let reason = TransportError::from(failure).to_string();
            if retryable {
                UpdateVerdict::Transient(reason)
            } else {
                UpdateVerdict::Refused { code: None, reason }
            }
        }
    }
}

/// A card edit: any refusal means the message cannot take it again.
fn update_result(
    response: std::result::Result<Value, http::Failure>,
) -> std::result::Result<(), EditFailure> {
    match update_verdict(response) {
        UpdateVerdict::Updated => Ok(()),
        UpdateVerdict::Transient(reason) => Err(EditFailure::Transient(reason)),
        UpdateVerdict::Refused { reason, .. } => Err(EditFailure::Gone(reason)),
    }
}

pub(crate) mod home;
pub(crate) mod threads;

#[cfg(test)]
mod tests;
