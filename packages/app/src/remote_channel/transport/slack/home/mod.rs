//! Slack's App Home: the page of the app's own on which daruda shows its
//! panes. Other transports have none; callers ask
//! [`crate::remote_channel::transport::app_home`] before using this.

use super::{API, Credentials, Result, TransportError, UpdateVerdict, refusal, verdict};
use crate::remote_channel::transport::{
    AppHome, HomeEvent, HomeEventKind, PageVerdict, PageView, chunks, http,
};
use serde_json::{Value, json};

/// The app home as daruda writes to it on Slack.
pub(crate) const APP_HOME: AppHome = AppHome {
    capacity: ROW_CAPACITY,
    publish,
    link: permalink,
    off_hint: "Turn on the Home tab in the Slack app's App Home settings",
};

/// The rows one page holds. Slack takes at most 100 blocks in a view: three
/// above the rows (header, the line a tap's result takes, divider), two per
/// row (its text, and one actions block for all its buttons), and one for
/// the rows left out.
const ROW_CAPACITY: usize = 48;
/// Slack's cap on a header's text.
const HEADER_LIMIT: usize = 150;
/// The cap this adapter keeps every other text to, as its messages do.
const TEXT_LIMIT: usize = super::SECTION_LIMIT;
/// The code `views.publish` answers with while the app's Home tab is off.
const NOT_ENABLED: &str = "not_enabled";

/// The person opening the app's Home tab, or tapping a button on it. An
/// opening is read from `payload.event` like a message; Slack's documents
/// disagree on that wrapping, so a parsed event, and any other event type
/// left unread, is traced to confirm it live.
pub(super) fn parse_home(envelope: &Value) -> Option<HomeEvent> {
    use crate::remote_channel::trace;
    if envelope["type"] == "interactive" {
        return parse_home_tap(envelope);
    }
    if envelope["type"] != "events_api" {
        return None;
    }
    let payload = &envelope["payload"];
    let event = &payload["event"];
    let kind = event["type"].as_str().unwrap_or_default();
    if kind != "app_home_opened" {
        if kind != "message" {
            trace::delivery("slack.event.unread", || {
                format!(
                    "event.type={kind:?} payload.type={:?}",
                    payload["type"].as_str().unwrap_or_default()
                )
            });
        }
        return None;
    }
    let tab = event["tab"].as_str().unwrap_or_default();
    trace::delivery("slack.home.opened", || {
        format!(
            "tab={tab:?} event_id={}",
            trace::opt(payload["event_id"].as_str())
        )
    });
    // The Messages tab is the paired conversation's own screen, not the page.
    if tab != "home" {
        return None;
    }
    Some(HomeEvent {
        event_id: payload["event_id"].as_str()?.to_owned(),
        user_id: event["user"].as_str()?.to_owned(),
        team_id: payload["team_id"].as_str()?.to_owned(),
        kind: HomeEventKind::Opened,
    })
}

/// A button tapped on the Home tab. Such a tap names no conversation or
/// message, and `team` is null for an org-wide install, so the person's own
/// `team_id` names the team. A link button carries no value: Slack opens the
/// link itself, and the acknowledgment already sent is all it needs.
fn parse_home_tap(envelope: &Value) -> Option<HomeEvent> {
    let payload = &envelope["payload"];
    if payload["type"] != "block_actions"
        || payload["container"]["type"] != "view"
        || payload["view"]["type"] != "home"
    {
        return None;
    }
    Some(HomeEvent {
        event_id: envelope["envelope_id"].as_str()?.to_owned(),
        user_id: payload["user"]["id"].as_str()?.to_owned(),
        team_id: payload["user"]["team_id"].as_str()?.to_owned(),
        kind: HomeEventKind::Tap {
            data: payload["actions"][0]["value"].as_str()?.to_owned(),
        },
    })
}

/// The view's blocks: a header, the last tap's result, then each pane's
/// text over its buttons.
fn page_blocks(view: &PageView<'_>) -> Vec<Value> {
    let mut blocks = vec![
        json!({"type": "header", "text": {"type": "plain_text", "text": capped(view.header, HEADER_LIMIT)}}),
    ];
    if let Some(outcome) = view.outcome {
        blocks.push(text_block(vec![plain(outcome)]));
    }
    blocks.push(json!({"type": "divider"}));
    if view.rows.is_empty() {
        blocks.push(text_block(vec![plain(view.empty)]));
    }
    for (index, row) in view.rows.iter().enumerate() {
        let mut elements = vec![
            json!({"type": "text", "text": capped(row.name, TEXT_LIMIT), "style": {"bold": true}}),
        ];
        if let Some(selected) = row.selected {
            elements.push(json!({"type": "text", "text": capped(&format!(" {selected}"), TEXT_LIMIT), "style": {"italic": true}}));
        }
        elements.push(json!({"type": "text", "text": "\n"}));
        if let Some(emoji) = row.emoji {
            elements.push(json!({"type": "emoji", "name": emoji}));
            elements.push(plain(&format!(" {}", row.status)));
        } else {
            elements.push(plain(row.status));
        }
        blocks.push(text_block(elements));
        let mut buttons = Vec::new();
        if let Some((label, url)) = row.thread {
            buttons.push(json!({
                "type": "button", "action_id": format!("home-thread-{index}"),
                "text": {"type": "plain_text", "text": capped(label, super::BUTTON_LABEL_LIMIT)},
                "url": url,
            }));
        }
        buttons.push(value_button(format!("home-select-{index}"), row.select));
        if let Some(stop) = row.stop {
            buttons.push(value_button(format!("home-stop-{index}"), stop));
        }
        blocks.push(json!({"type": "actions", "elements": buttons}));
    }
    if let Some(more) = view.more {
        blocks.push(text_block(vec![plain(more)]));
    }
    blocks
}

/// A button whose tap comes back to daruda carrying `value`.
fn value_button(action_id: String, (label, value): (&str, &str)) -> Value {
    json!({
        "type": "button", "action_id": action_id,
        "text": {"type": "plain_text", "text": capped(label, super::BUTTON_LABEL_LIMIT)},
        "value": value,
    })
}

/// A rich_text block, so a pane's name is shown literally: a `_` or `*` in
/// it stays itself.
fn text_block(elements: Vec<Value>) -> Value {
    json!({"type": "rich_text", "elements": [
        {"type": "rich_text_section", "elements": elements}]})
}

fn plain(text: &str) -> Value {
    json!({"type": "text", "text": capped(text, TEXT_LIMIT)})
}

fn capped(text: &str, limit: usize) -> String {
    chunks(text, limit).into_iter().next().unwrap_or_default()
}

/// Show `view` as `user_id`'s Home tab, replacing what it showed.
fn publish(credentials: &Credentials, user_id: &str, view: &PageView<'_>) -> PageVerdict {
    publish_verdict(http::request_with_status(
        "POST",
        &format!("{API}/views.publish"),
        Some(&format!("Bearer {}", credentials.bot)),
        &publish_payload(user_id, view),
    ))
}

fn publish_payload(user_id: &str, view: &PageView<'_>) -> Value {
    json!({"user_id": user_id, "view": {"type": "home", "blocks": page_blocks(view)}})
}

fn publish_verdict(response: std::result::Result<Value, http::Failure>) -> PageVerdict {
    match verdict("views.publish", response) {
        UpdateVerdict::Updated => PageVerdict::Published,
        UpdateVerdict::Transient(reason) => PageVerdict::Transient(reason),
        UpdateVerdict::Refused {
            code: Some(code), ..
        } if code == NOT_ENABLED => PageVerdict::Off,
        UpdateVerdict::Refused { reason, .. } => PageVerdict::Refused(reason),
    }
}

/// The link that opens message `message_ts` of `channel`: a thread's root
/// opens its thread.
fn permalink(credentials: &Credentials, channel: &str, message_ts: &str) -> Result<String> {
    // A read method: Slack takes its arguments in the query, not as JSON.
    let url = url::Url::parse_with_params(
        &format!("{API}/chat.getPermalink"),
        &[("channel", channel), ("message_ts", message_ts)],
    )
    .map_err(|_| TransportError::Message("Slack permalink request is invalid".into()))?;
    let response = http::request(
        "GET",
        url.as_str(),
        Some(&format!("Bearer {}", credentials.bot)),
        &Value::Null,
    )?;
    permalink_of(&response)
}

fn permalink_of(response: &Value) -> Result<String> {
    if let Some(code) = refusal(response) {
        return Err(TransportError::Message(format!(
            "Slack chat.getPermalink failed: {code}"
        )));
    }
    Ok(http::required(response, "permalink")?.to_owned())
}

#[cfg(test)]
mod tests;
