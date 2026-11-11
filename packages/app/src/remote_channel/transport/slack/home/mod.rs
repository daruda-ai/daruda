//! Slack's App Home: the page of the app's own on which daruda shows its
//! panes. Other transports have none; callers ask
//! [`crate::remote_channel::transport::app_home`] before using this.

use super::{API, Credentials, UpdateVerdict, verdict};
use crate::remote_channel::transport::{
    AppHome, HomeEvent, HomeEventKind, LinkFailure, PageBudget, PageHelp, PagePart, PageRow,
    PageVerdict, PageView, chunks, http,
};
use serde_json::{Value, json};

/// The app home as daruda writes to it on Slack.
pub(crate) const APP_HOME: AppHome = AppHome {
    budget: PageBudget {
        blocks: VIEW_BLOCKS - FRAME_BLOCKS,
        cards_per_block: CAROUSEL_CARDS,
        cards: CARD_CAPACITY,
    },
    publish,
    link: permalink,
    off_hint: "Turn on the Home tab in the Slack app's App Home settings",
};

/// Slack's cap on the blocks of one view.
const VIEW_BLOCKS: usize = 100;
/// The blocks of a page besides its listing, all at once: header, summary,
/// result, the two dividers, the line counting what is left out, help and
/// the status key. The empty-page line stands only where the listing would.
/// What is left, 92, is the listing's: one block per heading or label, one
/// carousel per ten cards.
const FRAME_BLOCKS: usize = 8;
/// The cards one page holds, a pane in the strip and in its project counted
/// twice. Blocks alone would allow hundreds; this keeps the payload and the
/// page a person scrolls through modest.
const CARD_CAPACITY: usize = 100;
/// Slack's cap on the cards of one carousel.
const CAROUSEL_CARDS: usize = 10;
/// Slack's cap on the elements of one context block.
const CONTEXT_ELEMENTS: usize = 10;
/// Slack's cap on a header's text.
const HEADER_LIMIT: usize = 150;
/// Slack's cap on a card's title and on its subtitle.
const CARD_TITLE_LIMIT: usize = 150;
/// Slack's cap on a card's body.
const CARD_BODY_LIMIT: usize = 200;
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
/// message. A link button carries no value: Slack opens the link itself,
/// and the acknowledgment already sent is all it needs.
fn parse_home_tap(envelope: &Value) -> Option<HomeEvent> {
    let payload = &envelope["payload"];
    if payload["type"] != "block_actions"
        || payload["container"]["type"] != "view"
        || payload["view"]["type"] != "home"
    {
        return None;
    }
    // The team a message tap names, so both are checked against one pairing;
    // `team` is null for an org-wide install, leaving only the person's own.
    let team = if payload["team"].is_null() {
        &payload["user"]["team_id"]
    } else {
        &payload["team"]["id"]
    };
    Some(HomeEvent {
        event_id: envelope["envelope_id"].as_str()?.to_owned(),
        user_id: payload["user"]["id"].as_str()?.to_owned(),
        team_id: team.as_str()?.to_owned(),
        kind: HomeEventKind::Tap {
            data: payload["actions"][0]["value"].as_str()?.to_owned(),
        },
    })
}

/// The view's blocks: a header over the counts and the last tap's result,
/// the listing's headings, labels and carousels of cards, then the usage
/// help. Only a pane's status shows an emoji: every text of daruda's or the
/// person's is written with shortcodes left as typed.
fn page_blocks(view: &PageView<'_>) -> Vec<Value> {
    let mut blocks = vec![json!({"type": "header", "text": plain(view.header, HEADER_LIMIT)})];
    for line in [view.summary, view.outcome].into_iter().flatten() {
        blocks.push(json!({"type": "context", "elements": [plain(line, TEXT_LIMIT)]}));
    }
    blocks.push(json!({"type": "divider"}));
    if view.parts.is_empty() {
        blocks.push(json!({"type": "section", "text": plain(view.empty, TEXT_LIMIT)}));
    }
    // Action ids are unique across the view: numbered on through every
    // carousel, a pane shown twice taking two.
    let mut index = 0;
    for part in &view.parts {
        match part {
            PagePart::Heading(text) => blocks.push(heading(text)),
            PagePart::Label(text) => {
                blocks.push(json!({"type": "context", "elements": [plain(text, TEXT_LIMIT)]}));
            }
            PagePart::Rows(rows) => {
                for cards in rows.chunks(CAROUSEL_CARDS) {
                    let elements: Vec<Value> = cards
                        .iter()
                        .map(|row| {
                            index += 1;
                            card(row, index - 1)
                        })
                        .collect();
                    blocks.push(json!({"type": "carousel", "elements": elements}));
                }
            }
        }
    }
    if let Some(more) = view.more {
        blocks.push(json!({"type": "context", "elements": [plain(more, TEXT_LIMIT)]}));
    }
    blocks.push(json!({"type": "divider"}));
    blocks.extend(help_blocks(&view.help));
    blocks
}

/// A heading in bold, as a thread root shows a pane's name: rich_text reads
/// no markup or shortcode in a group's or project's name.
fn heading(text: &str) -> Value {
    json!({"type": "rich_text", "elements": [{"type": "rich_text_section", "elements": [
        {"type": "text", "text": capped(text, TEXT_LIMIT), "style": {"bold": true}}]}]})
}

/// One pane: its title and subtitle as written, its status as the body (the
/// one emoji on the page), and up to Slack's three buttons a card holds.
fn card(row: &PageRow<'_>, index: usize) -> Value {
    let emoji = row
        .emoji
        .map(|name| format!(":{name}: "))
        .unwrap_or_default();
    let status = escaped(
        row.status,
        CARD_BODY_LIMIT.saturating_sub(emoji.encode_utf16().count()),
    );
    let mut buttons = Vec::new();
    if let Some((label, url)) = row.thread {
        buttons.push(json!({
            "type": "button", "action_id": format!("home-thread-{index}"),
            "text": plain(label, super::BUTTON_LABEL_LIMIT),
            "url": url,
        }));
    }
    buttons.push(value_button(format!("home-select-{index}"), row.select));
    if let Some(stop) = row.stop {
        buttons.push(value_button(format!("home-stop-{index}"), stop));
    }
    let mut card = json!({
        "type": "card",
        "title": plain(row.title, CARD_TITLE_LIMIT),
        "body": mrkdwn(&format!("{emoji}{status}")),
        "actions": buttons,
    });
    if let Some(subtitle) = row.subtitle {
        card["subtitle"] = plain(subtitle, CARD_TITLE_LIMIT);
    }
    card
}

/// The usage help as one section of lines, then the key to the status
/// emoji in a context block, one element per status.
fn help_blocks(help: &PageHelp<'_>) -> [Value; 2] {
    let mut text = format!("*{}*", escaped(help.title, TEXT_LIMIT));
    for line in &help.lines {
        text.push_str("\n• ");
        text.push_str(&escaped(line, TEXT_LIMIT));
    }
    let legend: Vec<Value> = help
        .legend
        .iter()
        .take(CONTEXT_ELEMENTS)
        .map(|(emoji, label)| mrkdwn(&format!(":{emoji}: {}", escaped(label, TEXT_LIMIT))))
        .collect();
    [
        json!({"type": "section", "text": mrkdwn(&capped(&text, TEXT_LIMIT))}),
        json!({"type": "context", "elements": legend}),
    ]
}

/// A button whose tap comes back to daruda carrying `value`.
fn value_button(action_id: String, (label, value): (&str, &str)) -> Value {
    json!({
        "type": "button", "action_id": action_id,
        "text": plain(label, super::BUTTON_LABEL_LIMIT),
        "value": value,
    })
}

/// Text shown as written: a `:name:` in a pane's name stays text.
fn plain(text: &str, limit: usize) -> Value {
    json!({"type": "plain_text", "text": capped(text, limit), "emoji": false})
}

fn mrkdwn(text: &str) -> Value {
    json!({"type": "mrkdwn", "text": text})
}

/// `text` with the three characters mrkdwn reserves escaped, cut whole
/// characters at a time so the escaped text stays within `limit`.
fn escaped(text: &str, limit: usize) -> String {
    let mut result = String::new();
    let mut size = 0;
    for ch in text.chars() {
        let piece = match ch {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            _ => ch.to_string(),
        };
        size += piece.encode_utf16().count();
        if size > limit {
            break;
        }
        result.push_str(&piece);
    }
    result
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

fn publish_verdict(response: Result<Value, http::Failure>) -> PageVerdict {
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
fn permalink(
    credentials: &Credentials,
    channel: &str,
    message_ts: &str,
) -> Result<String, LinkFailure> {
    // A read method: Slack takes its arguments in the query, not as JSON.
    let url = url::Url::parse_with_params(
        &format!("{API}/chat.getPermalink"),
        &[("channel", channel), ("message_ts", message_ts)],
    )
    .map_err(|_| LinkFailure::Refused("Slack permalink request is invalid".into()))?;
    permalink_of(http::request_with_status(
        "GET",
        url.as_str(),
        Some(&format!("Bearer {}", credentials.bot)),
        &Value::Null,
    ))
}

/// Read the link from a `chat.getPermalink` reply, telling a refusal of this
/// message from trouble that may pass as Slack's writes do.
fn permalink_of(response: Result<Value, http::Failure>) -> Result<String, LinkFailure> {
    let link = response
        .as_ref()
        .ok()
        .and_then(|value| http::required(value, "permalink").ok())
        .map(str::to_owned);
    match verdict("chat.getPermalink", response) {
        // A successful reply without the link is malformed, and the same
        // request gets the same answer again.
        UpdateVerdict::Updated => link.ok_or_else(|| {
            LinkFailure::Refused("Slack chat.getPermalink reply is missing permalink".into())
        }),
        UpdateVerdict::Transient(reason) => Err(LinkFailure::Transient(reason)),
        UpdateVerdict::Refused { reason, .. } => Err(LinkFailure::Refused(reason)),
    }
}

#[cfg(test)]
mod tests;
