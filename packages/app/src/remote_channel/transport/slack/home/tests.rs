use super::super::{BUTTON_LABEL_LIMIT, parse};
use super::*;
use crate::remote_channel::transport::{IncomingKind, PagePart, PageRow};

fn row<'a>(title: &'a str, emoji: Option<&'a str>, thread: Option<&'a str>) -> PageRow<'a> {
    PageRow {
        title,
        subtitle: None,
        emoji,
        status: "Working",
        thread: thread.map(|url| ("Open thread", url)),
        select: ("Select", "hom:1:0:sel"),
        stop: None,
    }
}

const LEGEND: [(&str, &str); 2] = [("hourglass_flowing_sand", "Working"), ("bell", "Needs you")];

fn view<'a>(parts: Vec<PagePart<'a>>, more: Option<&'a str>) -> PageView<'a> {
    PageView {
        header: "Agents",
        summary: Some("Working 1 · Needs you 0 · Total 2"),
        outcome: None,
        empty: "No agent chats are open.",
        parts,
        more,
        help: PageHelp {
            title: "How to use",
            lines: vec!["Reply in a thread.", "Commands: `!list` · `!stop`"],
            legend: LEGEND.to_vec(),
        },
    }
}

/// A heading over `rows`, as the strip or an ungrouped project is shown.
fn group<'a>(title: &'a str, rows: Vec<PageRow<'a>>) -> [PagePart<'a>; 2] {
    [PagePart::Heading(title), PagePart::Rows(rows)]
}

/// A heading as the page draws it: bold, literal text.
fn heading_text(block: &Value) -> &str {
    assert_eq!(block["type"], "rich_text", "{block}");
    let text = &block["elements"][0]["elements"][0];
    assert_eq!(text["type"], "text");
    assert_eq!(text["style"], json!({"bold": true}));
    text["text"].as_str().unwrap()
}

fn types(blocks: &[Value]) -> Vec<&str> {
    blocks
        .iter()
        .map(|block| block["type"].as_str().unwrap())
        .collect()
}

/// Every card of the view, in order.
fn cards(blocks: &[Value]) -> Vec<&Value> {
    blocks
        .iter()
        .filter(|block| block["type"] == "carousel")
        .flat_map(|block| block["elements"].as_array().unwrap())
        .collect()
}

/// Header, counts, divider, then the listing in order: a heading in bold
/// rich text, a label as context, each run of rows as a carousel; the rows
/// left out, divider, help, and the status key.
#[test]
fn a_page_is_laid_out_header_listing_then_help() {
    let mut asking = row("web/fix (Claude)", Some("bell"), None);
    asking.stop = Some(("Stop", "hom:1:3:stop"));
    let mut again = row("web/fix (Claude)", Some("bell"), None);
    again.stop = Some(("Stop", "hom:1:3:stop"));
    let quiet = row("cli/main (Codex)", None, None);
    let page = view(
        vec![
            PagePart::Heading("Needs you"),
            PagePart::Rows(vec![asking]),
            PagePart::Heading("Window 1"),
            PagePart::Heading("tools"),
            PagePart::Label("cli"),
            PagePart::Rows(vec![quiet]),
            PagePart::Label("web/fix"),
            PagePart::Rows(vec![again]),
        ],
        Some("2 more not shown"),
    );
    let payload = publish_payload("U1", &page);
    assert_eq!(payload["user_id"], "U1");
    assert_eq!(payload["view"]["type"], "home");
    let blocks = payload["view"]["blocks"].as_array().unwrap();
    assert_eq!(
        types(blocks),
        [
            "header",
            "context",
            "divider",
            "rich_text",
            "carousel",
            "rich_text",
            "rich_text",
            "context",
            "carousel",
            "context",
            "carousel",
            "context",
            "divider",
            "section",
            "context"
        ]
    );
    assert_eq!(blocks[0]["text"]["text"], "Agents");
    assert_eq!(
        blocks[1]["elements"][0]["text"],
        "Working 1 · Needs you 0 · Total 2"
    );
    assert_eq!(heading_text(&blocks[3]), "Needs you");
    assert_eq!(
        blocks[4]["elements"][0]["title"]["text"],
        "web/fix (Claude)"
    );
    assert_eq!(heading_text(&blocks[5]), "Window 1");
    assert_eq!(heading_text(&blocks[6]), "tools");
    assert_eq!(
        blocks[7]["elements"],
        json!([{"type": "plain_text", "text": "cli", "emoji": false}])
    );
    assert_eq!(
        blocks[8]["elements"][0]["title"]["text"],
        "cli/main (Codex)"
    );
    assert_eq!(blocks[9]["elements"][0]["text"], "web/fix");
    // The pane shown twice: the same values, its own action ids.
    let (first, second) = (&blocks[4]["elements"][0], &blocks[10]["elements"][0]);
    assert_eq!(first["actions"][1]["value"], second["actions"][1]["value"]);
    assert_eq!(first["actions"][0]["action_id"], "home-select-0");
    assert_eq!(second["actions"][0]["action_id"], "home-select-2");
    assert_eq!(blocks[11]["elements"][0]["text"], "2 more not shown");
    assert_eq!(
        blocks[13]["text"]["text"],
        "*How to use*\n• Reply in a thread.\n• Commands: `!list` · `!stop`"
    );
    // The key is built from the statuses the page was handed.
    assert_eq!(
        blocks[14]["elements"],
        json!([
            {"type": "mrkdwn", "text": ":hourglass_flowing_sand: Working"},
            {"type": "mrkdwn", "text": ":bell: Needs you"},
        ])
    );
}

/// A tap's result follows the counts.
#[test]
fn a_tap_result_follows_the_counts() {
    let mut page = view(group("app", vec![row("a", None, None)]).into(), None);
    page.outcome = Some("09:30 · Stopped");
    let blocks = page_blocks(&page);
    assert_eq!(
        types(&blocks)[..6],
        [
            "header",
            "context",
            "context",
            "divider",
            "rich_text",
            "carousel"
        ]
    );
    assert_eq!(blocks[2]["elements"][0]["text"], "09:30 · Stopped");
    assert_eq!(heading_text(&blocks[4]), "app");
}

#[test]
fn an_empty_page_says_so_and_still_helps() {
    let mut page = view(Vec::new(), None);
    page.summary = None;
    let blocks = page_blocks(&page);
    assert_eq!(
        types(&blocks),
        [
            "header", "divider", "section", "divider", "section", "context"
        ]
    );
    assert_eq!(blocks[2]["text"]["text"], "No agent chats are open.");
}

/// A card is titled by the pane's conversation, subtitled by its agent as
/// plain text, and reads the status with its emoji as its body; it carries a
/// link only when one is known, Stop only on a running pane.
#[test]
fn a_card_shows_one_pane_and_its_buttons() {
    let mut running = row(
        "Fix the login bug",
        Some("hourglass_flowing_sand"),
        Some("https://x.slack.com/archives/D1/p1"),
    );
    running.stop = Some(("Stop", "hom:1:0:stop"));
    running.subtitle = Some("Claude Code");
    running.status = "Working since 09:30 · Selected";
    let mut quiet = row("Codex", None, None);
    quiet.select = ("Select", "hom:1:1:sel");
    quiet.status = "Idle";
    let blocks = page_blocks(&view(
        vec![PagePart::Rows(vec![running]), PagePart::Rows(vec![quiet])],
        None,
    ));
    let cards = cards(&blocks);
    assert_eq!(
        *cards[0],
        json!({
            "type": "card",
            "title": {"type": "plain_text", "text": "Fix the login bug", "emoji": false},
            "subtitle": {"type": "plain_text", "text": "Claude Code", "emoji": false},
            "body": {"type": "mrkdwn",
                "text": ":hourglass_flowing_sand: Working since 09:30 · Selected"},
            "actions": [
                {"type": "button", "action_id": "home-thread-0",
                    "text": {"type": "plain_text", "text": "Open thread", "emoji": false},
                    "url": "https://x.slack.com/archives/D1/p1"},
                {"type": "button", "action_id": "home-select-0",
                    "text": {"type": "plain_text", "text": "Select", "emoji": false},
                    "value": "hom:1:0:sel"},
                {"type": "button", "action_id": "home-stop-0",
                    "text": {"type": "plain_text", "text": "Stop", "emoji": false},
                    "value": "hom:1:0:stop"},
            ],
        })
    );
    assert_eq!(cards[1]["title"]["text"], "Codex");
    assert_eq!(cards[1]["body"]["text"], "Idle", "no status, no emoji");
    assert!(cards[1].get("subtitle").is_none(), "the title is the agent");
    assert_eq!(
        cards[1]["actions"],
        json!([{"type": "button", "action_id": "home-select-1",
            "text": {"type": "plain_text", "text": "Select", "emoji": false},
            "value": "hom:1:1:sel"}]),
        "no link known, nothing to stop"
    );
}

/// Slack holds ten cards to a carousel: a run of 23 panes takes three, as
/// the budget counts it.
#[test]
fn a_run_of_rows_is_split_into_carousels_of_ten() {
    let rows = (0..23).map(|_| row("pane", None, None)).collect();
    let blocks = page_blocks(&view(vec![PagePart::Rows(rows)], None));
    assert_eq!(
        blocks
            .iter()
            .filter(|block| block["type"] == "carousel")
            .count(),
        APP_HOME.budget.rows_blocks(23)
    );
    let sizes: Vec<usize> = blocks
        .iter()
        .filter(|block| block["type"] == "carousel")
        .map(|block| block["elements"].as_array().unwrap().len())
        .collect();
    assert_eq!(sizes, [10, 10, 3]);
    let ids: Vec<&str> = cards(&blocks)
        .iter()
        .map(|card| card["actions"][0]["action_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids[0], "home-select-0");
    assert_eq!(ids[22], "home-select-22", "numbered on across carousels");
}

/// A listing that takes the whole budget, a heading or label a block and a
/// carousel per ten cards, under a result line and over the rows left out,
/// fills Slack's 100 blocks and no more; every button keeps its own action
/// id, a pane shown twice included.
#[test]
fn a_full_page_fits_in_one_view() {
    let budget = APP_HOME.budget;
    let full = || {
        let mut row = row("pane", Some("bell"), Some("https://x"));
        row.stop = Some(("Stop", "hom:1:0:stop"));
        row.subtitle = Some("subtitle");
        row
    };
    let runs = budget.cards / CAROUSEL_CARDS;
    let mut parts: Vec<PagePart<'_>> = (0..runs)
        .map(|_| PagePart::Rows((0..CAROUSEL_CARDS).map(|_| full()).collect()))
        .collect();
    let used = runs * budget.rows_blocks(CAROUSEL_CARDS);
    parts.extend((used..budget.blocks).map(|_| PagePart::Label("project")));
    let mut page = view(parts, Some("more"));
    page.outcome = Some("09:30 · Stopped");
    let blocks = page_blocks(&page);
    assert_eq!(blocks.len(), 100, "the frame and a full listing");
    let cards = cards(&blocks);
    assert_eq!(cards.len(), budget.cards);
    assert!(
        cards
            .iter()
            .all(|card| card["actions"].as_array().unwrap().len() <= 3)
    );
    let action_ids: std::collections::HashSet<_> = cards
        .iter()
        .flat_map(|card| card["actions"].as_array().unwrap())
        .filter_map(|button| button["action_id"].as_str())
        .collect();
    assert_eq!(action_ids.len(), budget.cards * 3);
}

/// Only a status shows an emoji: a shortcode in a pane's, project's or
/// group's name, its subject or daruda's own texts stays text, and no
/// mrkdwn text but a card's status and the status key carries one.
#[test]
fn only_statuses_show_emoji() {
    let mut odd = row(":fire: app", Some("bell"), None);
    odd.subtitle = Some(":tada: done");
    let blocks = page_blocks(&view(
        vec![
            PagePart::Heading(":rocket: *group*"),
            PagePart::Label(":star: project"),
            PagePart::Rows(vec![odd]),
        ],
        None,
    ));
    assert_eq!(heading_text(&blocks[3]), ":rocket: *group*", "literal");
    let mut shown = Vec::new();
    walk(&Value::from(blocks.clone()), &mut |text: &Value| {
        if text["type"] == "plain_text" {
            assert_eq!(text["emoji"], false, "{text}");
        } else if text["type"] == "mrkdwn" {
            let body = text["text"].as_str().unwrap();
            if has_shortcode(body) {
                shown.push(body.to_owned());
            }
        }
    });
    assert_eq!(
        shown,
        [
            ":bell: Working",
            ":hourglass_flowing_sand: Working",
            ":bell: Needs you"
        ]
    );
}

/// Whether `text` holds a `:name:` emoji shortcode.
fn has_shortcode(text: &str) -> bool {
    text.split(':')
        .skip(1)
        .collect::<Vec<_>>()
        .split_last()
        .is_some_and(|(_, inner)| {
            inner.iter().any(|name| {
                !name.is_empty()
                    && name.chars().all(|ch| {
                        ch.is_ascii_lowercase() || ch.is_ascii_digit() || "_+-".contains(ch)
                    })
            })
        })
}

/// Calls `visit` on every text object in `value`.
fn walk(value: &Value, visit: &mut impl FnMut(&Value)) {
    match value {
        Value::Object(map) => {
            if map.contains_key("text")
                && map
                    .get("type")
                    .is_some_and(|kind| kind == "plain_text" || kind == "mrkdwn")
            {
                visit(value);
            }
            map.values().for_each(|child| walk(child, visit));
        }
        Value::Array(items) => items.iter().for_each(|child| walk(child, visit)),
        _ => {}
    }
}

/// Texts are cut whole characters at a time to Slack's caps, and mrkdwn's
/// reserved characters are escaped within them.
#[test]
fn long_texts_are_capped() {
    let name = "n".repeat(CARD_TITLE_LIMIT + 10);
    let subtitle = "s".repeat(CARD_TITLE_LIMIT + 10);
    let status = "<".repeat(CARD_BODY_LIMIT);
    let label = "l".repeat(100);
    let mut long = row(&name, Some("bell"), Some("https://x"));
    long.subtitle = Some(&subtitle);
    long.status = &status;
    long.thread = Some((&label, "https://x"));
    let header = "h".repeat(200);
    let mut page = view(group("Running", vec![long]).into(), None);
    page.header = &header;
    let blocks = page_blocks(&page);
    let length = |value: &Value| value.as_str().unwrap().encode_utf16().count();
    assert_eq!(length(&blocks[0]["text"]["text"]), HEADER_LIMIT);
    let card = cards(&blocks)[0];
    assert_eq!(length(&card["title"]["text"]), CARD_TITLE_LIMIT);
    assert_eq!(length(&card["subtitle"]["text"]), CARD_TITLE_LIMIT);
    let body = card["body"]["text"].as_str().unwrap();
    assert!(body.encode_utf16().count() <= CARD_BODY_LIMIT, "{body}");
    assert!(
        body.trim_start_matches(":bell: ")
            .split_inclusive(';')
            .all(|piece| piece == "&lt;"),
        "escaped whole: {body}"
    );
    assert_eq!(
        length(&card["actions"][0]["text"]["text"]),
        BUTTON_LABEL_LIMIT
    );
}

/// A subtitle is plain text: a shortcode in it stays text, and a card
/// without one has no subtitle at all.
#[test]
fn a_subtitle_is_plain_text_and_optional() {
    let mut strip = row("Fix the bug", None, None);
    strip.subtitle = Some("app · Claude Code");
    let blocks = page_blocks(&view(
        vec![PagePart::Rows(vec![strip, row("Codex", None, None)])],
        None,
    ));
    let cards = cards(&blocks);
    assert_eq!(
        cards[0]["subtitle"],
        json!({"type": "plain_text", "text": "app · Claude Code", "emoji": false})
    );
    assert!(cards[1].get("subtitle").is_none());
}

#[test]
fn a_publish_reply_is_classified_by_what_it_says_of_the_page() {
    use crate::remote_channel::transport::http::Failure;
    let error = |code: &str| Ok(json!({"ok": false, "error": code}));
    let cases = [
        (Ok(json!({"ok": true})), "published"),
        (error("not_enabled"), "off"),
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
            PageVerdict::Published => "published",
            PageVerdict::Off => "off",
            PageVerdict::Refused(_) => "refused",
            PageVerdict::Transient(_) => "transient",
        };
        assert_eq!(got, want, "{shown}");
    }
}

/// A permalink is read from the reply. A refusal of this message is told
/// apart from trouble that may pass, so the page stops asking only for the
/// former.
#[test]
fn a_permalink_is_read_from_the_reply_or_refused_by_code() {
    use crate::remote_channel::transport::LinkFailure;
    let transient = |reason: &str| Err(LinkFailure::Transient(reason.into()));
    let refused = |reason: &str| Err(LinkFailure::Refused(reason.into()));
    let cases = [
        (
            Ok(json!({"ok": true, "permalink": "https://x/p1"})),
            Ok("https://x/p1".to_owned()),
            "the link",
        ),
        (
            Ok(json!({"ok": false, "error": "message_not_found"})),
            refused("Slack chat.getPermalink failed: message_not_found"),
            "a message Slack does not know",
        ),
        (
            Ok(json!({"ok": false, "error": "ratelimited"})),
            transient("Slack chat.getPermalink failed: ratelimited"),
            "a documented passing code",
        ),
        (
            Ok(json!({"ok": false})),
            transient("Slack chat.getPermalink failed: unknown_error"),
            "a refusal that names no code",
        ),
        (
            Ok(json!({"ok": true})),
            refused("Slack chat.getPermalink reply is missing permalink"),
            "a successful reply without the link, malformed alike each time",
        ),
        (
            Err(http::Failure::Status(503)),
            transient("Remote API returned HTTP 503"),
            "server trouble",
        ),
        (
            Err(http::Failure::Status(403)),
            refused("Remote API returned HTTP 403"),
            "a status that will not pass",
        ),
    ];
    for (response, want, label) in cases {
        assert_eq!(permalink_of(response), want, "{label}");
    }
}

/// Opening the Home tab names the person and team; the Messages tab and
/// ordinary messages are not the page.
#[test]
fn opening_the_home_tab_is_read_from_the_event_and_nothing_else_is() {
    let opened = |tab: &str| {
        json!({"type":"events_api", "payload":{"event_id":"Ev3", "team_id":"T1",
            "event":{"type":"app_home_opened", "user":"U1", "channel":"D1", "tab":tab}}})
    };
    let home = parse_home(&opened("home")).expect("the Home tab is the page");
    assert_eq!(
        (
            home.event_id.as_str(),
            home.user_id.as_str(),
            home.team_id.as_str()
        ),
        ("Ev3", "U1", "T1")
    );
    assert!(matches!(home.kind, HomeEventKind::Opened));
    let message = json!({"type":"events_api", "payload":{"event_id":"Ev1", "team_id":"T1",
        "event":{"type":"message", "user":"U1", "channel":"D1", "text":"hi"}}});
    let cases = [
        (opened("messages"), "the Messages tab"),
        (message.clone(), "a message"),
        (
            json!({"type":"interactive", "payload":{"type":"block_actions"}}),
            "a tap",
        ),
    ];
    for (envelope, label) in cases {
        assert!(parse_home(&envelope).is_none(), "{label}");
    }
    assert!(parse(&message).is_some(), "a message still parses as one");
    assert!(parse(&opened("home")).is_none(), "the page is no message");
}

/// A Home-tab `block_actions` payload as Slack sends it: a view container,
/// no channel, message or response URL, and a null `team` on an org install.
fn home_tap(action: Value) -> Value {
    json!({"type":"interactive", "envelope_id":"env-7", "payload":{
        "type":"block_actions",
        "user":{"id":"U1", "username":"woo", "name":"woo", "team_id":"T1"},
        "api_app_id":"A1", "token":"x", "trigger_id":"1.2.3",
        "container":{"type":"view", "view_id":"V1"},
        "team":null,
        "enterprise":{"id":"E1", "name":"org"},
        "is_enterprise_install":true,
        "view":{"id":"V1", "type":"home", "team_id":"T1", "app_id":"A1",
            "bot_id":"B1", "blocks":[], "state":{"values":{}}, "hash":"h"},
        "actions":[action],
    }})
}

/// A tap on a page button names the person, their own team and the value;
/// a link button is Slack's to open; a tap on a message stays a callback.
#[test]
fn a_home_tap_is_read_apart_from_a_message_tap() {
    let tapped = parse_home(&home_tap(json!({"action_id":"home-stop-0", "block_id":"b",
        "type":"button", "value":"hom:7:0:stop", "action_ts":"1.2"})))
    .expect("a page button");
    assert_eq!(
        (
            tapped.event_id.as_str(),
            tapped.user_id.as_str(),
            tapped.team_id.as_str()
        ),
        ("env-7", "U1", "T1")
    );
    assert!(matches!(tapped.kind, HomeEventKind::Tap { data } if data == "hom:7:0:stop"));

    let link = home_tap(json!({"action_id":"home-thread-0", "block_id":"b",
        "type":"button", "url":"https://x", "action_ts":"1.2"}));
    assert!(
        parse_home(&link).is_none(),
        "a link button carries no value"
    );
    assert!(parse(&link).is_none(), "nor is it a message callback");

    let mut modal = home_tap(json!({"type":"button", "value":"hom:7:0:stop"}));
    modal["payload"]["view"]["type"] = json!("modal");
    assert!(
        parse_home(&modal).is_none(),
        "only the Home tab is the page"
    );

    let message = json!({"type":"interactive", "envelope_id":"e1", "payload":{
        "type":"block_actions", "user":{"id":"U2"}, "channel":{"id":"D1"}, "team":{"id":"T1"},
        "container":{"type":"message", "message_ts":"12.34"},
        "message":{"ts":"12.34", "text":"permission"}, "actions":[{"value":"opaque-token"}]
    }});
    assert!(
        parse_home(&message).is_none(),
        "a message tap is no page tap"
    );
    let callback = parse(&message).expect("a message tap");
    assert!(matches!(callback.kind, IncomingKind::Callback { data, .. } if data == "opaque-token"));
}

/// A page tap names the workspace a message tap names; only an org-wide
/// install, whose `team` is null, leaves the person's own team to name it.
#[test]
fn a_home_tap_names_the_team_a_message_tap_names() {
    let action = json!({"type":"button", "value":"hom:7:0:stop"});
    let team = |envelope: &Value| parse_home(envelope).map(|tap| tap.team_id);
    let org_wide = home_tap(action.clone());
    assert_eq!(team(&org_wide).as_deref(), Some("T1"), "the person's own");
    let mut workspace = home_tap(action);
    workspace["payload"]["team"] = json!({"id":"T2", "domain":"other"});
    assert_eq!(
        team(&workspace).as_deref(),
        Some("T2"),
        "the payload's team, as a message tap reads it"
    );
}
