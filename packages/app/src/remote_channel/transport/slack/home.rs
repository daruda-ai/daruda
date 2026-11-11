//! Slack's App Home: the page of the app's own on which daruda shows its
//! panes. Other transports have none; callers ask
//! [`crate::remote_channel::transport::app_home`] before using this.

use super::{API, Credentials, Result, TransportError, UpdateVerdict, refusal, verdict};
use crate::remote_channel::transport::{chunks, http};
use serde_json::{Value, json};

/// The rows one page holds. Slack takes at most 100 blocks in a view: three
/// above the rows (header, the line a tap's result takes, divider), two per
/// row (its text, and one actions block for all its buttons), and one for
/// the rows left out.
pub(crate) const ROW_CAPACITY: usize = 48;
/// Slack's cap on a header's text.
const HEADER_LIMIT: usize = 150;
/// The cap this adapter keeps every other text to, as its messages do.
const TEXT_LIMIT: usize = super::SECTION_LIMIT;
/// The code `views.publish` answers with while the app's Home tab is off.
const NOT_ENABLED: &str = "not_enabled";

/// What a page shows, in the order it shows it.
pub(crate) struct HomeView<'a> {
    pub header: &'a str,
    /// What the person's last tap came to, under the header.
    pub outcome: Option<&'a str>,
    /// Said in place of the rows when there are none.
    pub empty: &'a str,
    pub rows: Vec<RowView<'a>>,
    /// The line that counts the rows left out, when some were.
    pub more: Option<&'a str>,
}

/// One pane on the page: its name in bold, its status below, its buttons
/// under both.
pub(crate) struct RowView<'a> {
    pub name: &'a str,
    /// The mark beside the name of the pane commands address.
    pub selected: Option<&'a str>,
    /// The status's Slack emoji name, when one is known.
    pub emoji: Option<&'a str>,
    pub status: &'a str,
    /// The button's label and the link to the pane's thread.
    pub thread: Option<(&'a str, &'a str)>,
    /// The select button's label and value.
    pub select: (&'a str, &'a str),
    /// The stop button's label and value, when the pane can be stopped.
    pub stop: Option<(&'a str, &'a str)>,
}

impl HomeView<'_> {
    fn blocks(&self) -> Vec<Value> {
        let mut blocks = vec![
            json!({"type": "header", "text": {"type": "plain_text", "text": capped(self.header, HEADER_LIMIT)}}),
        ];
        if let Some(outcome) = self.outcome {
            blocks.push(text_block(vec![plain(outcome)]));
        }
        blocks.push(json!({"type": "divider"}));
        if self.rows.is_empty() {
            blocks.push(text_block(vec![plain(self.empty)]));
        }
        for (index, row) in self.rows.iter().enumerate() {
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
        if let Some(more) = self.more {
            blocks.push(text_block(vec![plain(more)]));
        }
        blocks
    }
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

/// How a `views.publish` went.
#[derive(Debug)]
pub(crate) enum PublishVerdict {
    Published,
    /// The app's Home tab is off: no page will take a view until it is on.
    Disabled,
    /// Rate limits, timeouts, server trouble: may pass.
    Transient(String),
    /// Slack refused this view: credentials, scopes, or the payload.
    Refused(String),
}

/// Show `view` as `user_id`'s Home tab, replacing what it showed.
pub(crate) fn publish(
    credentials: &Credentials,
    user_id: &str,
    view: &HomeView<'_>,
) -> PublishVerdict {
    publish_verdict(http::request_with_status(
        "POST",
        &format!("{API}/views.publish"),
        Some(&format!("Bearer {}", credentials.bot)),
        &publish_payload(user_id, view),
    ))
}

fn publish_payload(user_id: &str, view: &HomeView<'_>) -> Value {
    json!({"user_id": user_id, "view": {"type": "home", "blocks": view.blocks()}})
}

fn publish_verdict(response: std::result::Result<Value, http::Failure>) -> PublishVerdict {
    match verdict("views.publish", response) {
        UpdateVerdict::Updated => PublishVerdict::Published,
        UpdateVerdict::Transient(reason) => PublishVerdict::Transient(reason),
        UpdateVerdict::Refused {
            code: Some(code), ..
        } if code == NOT_ENABLED => PublishVerdict::Disabled,
        UpdateVerdict::Refused { reason, .. } => PublishVerdict::Refused(reason),
    }
}

/// The link that opens message `message_ts` of `channel`: a thread's root
/// opens its thread.
pub(crate) fn permalink(
    credentials: &Credentials,
    channel: &str,
    message_ts: &str,
) -> Result<String> {
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
mod tests {
    use super::*;

    fn row<'a>(name: &'a str, emoji: Option<&'a str>, thread: Option<&'a str>) -> RowView<'a> {
        RowView {
            name,
            selected: None,
            emoji,
            status: "Working",
            thread: thread.map(|url| ("Open thread", url)),
            select: ("Select", "hom:1:0:sel"),
            stop: None,
        }
    }

    fn view<'a>(rows: Vec<RowView<'a>>, more: Option<&'a str>) -> HomeView<'a> {
        HomeView {
            header: "Agents",
            outcome: None,
            empty: "No agent chats are open.",
            rows,
            more,
        }
    }

    #[test]
    fn a_page_lists_each_pane_with_its_status_and_buttons() {
        let mut running = row(
            "app/main (Claude)",
            Some("hourglass_flowing_sand"),
            Some("https://x.slack.com/archives/D1/p1"),
        );
        running.stop = Some(("Stop", "hom:1:0:stop"));
        let mut quiet = row("app/fix_bug (Codex)", None, None);
        quiet.select = ("Select", "hom:1:1:sel");
        quiet.selected = Some("(selected)");
        let payload = publish_payload("U1", &view(vec![running, quiet], Some("…and 2 more")));
        assert_eq!(payload["user_id"], "U1");
        assert_eq!(payload["view"]["type"], "home");
        let blocks = payload["view"]["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[0]["text"]["text"], "Agents");
        assert_eq!(blocks[1]["type"], "divider", "no tap, no result line");
        let first = &blocks[2]["elements"][0]["elements"];
        assert_eq!(first[0]["text"], "app/main (Claude)");
        assert_eq!(first[0]["style"]["bold"], true);
        assert_eq!(
            first[2],
            json!({"type": "emoji", "name": "hourglass_flowing_sand"})
        );
        assert_eq!(first[3]["text"], " Working");
        let buttons = blocks[3]["elements"].as_array().unwrap();
        assert_eq!(buttons[0]["url"], "https://x.slack.com/archives/D1/p1");
        assert_eq!(buttons[0]["text"]["text"], "Open thread");
        assert!(buttons[0].get("value").is_none(), "a link carries no token");
        assert_eq!(buttons[1]["value"], "hom:1:0:sel");
        assert_eq!(buttons[1]["text"]["text"], "Select");
        assert_eq!(buttons[2]["value"], "hom:1:0:stop");
        assert_eq!(buttons[2]["text"]["text"], "Stop");
        let second = &blocks[4]["elements"][0]["elements"];
        assert_eq!(
            second[1]["text"], " (selected)",
            "the selected pane is marked"
        );
        assert_eq!(second[3]["text"], "Working", "no emoji without a status");
        let buttons = blocks[5]["elements"].as_array().unwrap();
        assert_eq!(buttons.len(), 1, "no thread link, nothing to stop");
        assert_eq!(buttons[0]["value"], "hom:1:1:sel");
        assert_eq!(
            blocks[6]["elements"][0]["elements"][0]["text"],
            "…and 2 more"
        );
        assert_eq!(blocks.len(), 7);
        let action_ids: std::collections::HashSet<_> = blocks
            .iter()
            .filter_map(|block| block["elements"].as_array())
            .flatten()
            .filter_map(|element| element["action_id"].as_str())
            .collect();
        assert_eq!(action_ids.len(), 4, "every button has its own action id");
    }

    #[test]
    fn a_tap_result_sits_between_the_header_and_the_rows() {
        let mut page = view(Vec::new(), None);
        page.outcome = Some("09:30 · Stopped");
        let blocks = page.blocks();
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(
            blocks[1]["elements"][0]["elements"][0]["text"],
            "09:30 · Stopped"
        );
        assert_eq!(blocks[2]["type"], "divider");
    }

    #[test]
    fn an_empty_page_says_so() {
        let payload = publish_payload("U1", &view(Vec::new(), None));
        let blocks = payload["view"]["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(
            blocks[2]["elements"][0]["elements"][0]["text"],
            "No agent chats are open."
        );
    }

    /// A full page, every row with all three buttons, a result line, and
    /// the rows left out counted, stays within Slack's 100 blocks.
    #[test]
    fn a_full_page_fits_in_one_view() {
        let rows = (0..ROW_CAPACITY)
            .map(|_| {
                let mut row = row("pane", Some("bell"), Some("https://x"));
                row.stop = Some(("Stop", "hom:1:0:stop"));
                row.selected = Some("(selected)");
                row
            })
            .collect();
        let mut page = view(rows, Some("more"));
        page.outcome = Some("09:30 · Stopped");
        let count = publish_payload("U1", &page)["view"]["blocks"]
            .as_array()
            .unwrap()
            .len();
        assert!(count <= 100, "{count} blocks");
    }

    #[test]
    fn long_texts_are_capped() {
        let name = "n".repeat(TEXT_LIMIT + 10);
        let mut long = row(&name, None, Some("https://x"));
        let label = "l".repeat(100);
        long.thread = Some((&label, "https://x"));
        let header = "h".repeat(200);
        let mut page = view(vec![long], None);
        page.header = &header;
        let blocks = page.blocks();
        let length = |value: &Value| value.as_str().unwrap().chars().count();
        assert_eq!(length(&blocks[0]["text"]["text"]), HEADER_LIMIT);
        assert_eq!(
            length(&blocks[2]["elements"][0]["elements"][0]["text"]),
            TEXT_LIMIT
        );
        assert_eq!(
            length(&blocks[3]["elements"][0]["text"]["text"]),
            super::super::BUTTON_LABEL_LIMIT
        );
    }

    #[test]
    fn a_publish_reply_is_classified_by_what_it_says_of_the_page() {
        use super::super::http::Failure;
        let error = |code: &str| Ok(json!({"ok": false, "error": code}));
        let cases = [
            (Ok(json!({"ok": true})), "published"),
            (error("not_enabled"), "disabled"),
            (error("invalid_blocks"), "refused"),
            (error("missing_scope"), "refused"),
            (error("ratelimited"), "transient"),
            (Ok(json!({"ok": false})), "transient"),
            (Err(Failure::Status(503)), "transient"),
            (Err(Failure::Status(403)), "refused"),
        ];
        for (response, want) in cases {
            let shown = format!("{response:?}");
            let got = match publish_verdict(response) {
                PublishVerdict::Published => "published",
                PublishVerdict::Disabled => "disabled",
                PublishVerdict::Refused(_) => "refused",
                PublishVerdict::Transient(_) => "transient",
            };
            assert_eq!(got, want, "{shown}");
        }
    }

    #[test]
    fn a_permalink_is_read_from_the_reply_or_refused_by_code() {
        let link = permalink_of(&json!({"ok": true, "permalink": "https://x/p1"}));
        assert_eq!(link.unwrap(), "https://x/p1");
        let refused = permalink_of(&json!({"ok": false, "error": "message_not_found"}));
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("message_not_found")
        );
        assert!(permalink_of(&json!({"ok": true})).is_err(), "no link");
    }
}
