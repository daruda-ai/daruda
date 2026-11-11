use super::super::{BUTTON_LABEL_LIMIT, parse};
use super::*;
use crate::remote_channel::transport::{IncomingKind, PageRow};

fn row<'a>(name: &'a str, emoji: Option<&'a str>, thread: Option<&'a str>) -> PageRow<'a> {
    PageRow {
        name,
        subject: None,
        selected: None,
        emoji,
        status: "Working",
        thread: thread.map(|url| ("Open thread", url)),
        select: ("Select", "hom:1:0:sel"),
        stop: None,
    }
}

fn view<'a>(rows: Vec<PageRow<'a>>, more: Option<&'a str>) -> PageView<'a> {
    PageView {
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
    running.subject = Some("Fix the login bug");
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
    // Read as the pane's thread root reads: bold name, subject, status.
    assert_eq!(
        blocks[2]["elements"][0]["elements"],
        json!([
            {"type": "text", "text": "app/main (Claude)", "style": {"bold": true}},
            {"type": "text", "text": "\nFix the login bug"},
            {"type": "text", "text": "\n"},
            {"type": "emoji", "name": "hourglass_flowing_sand"},
            {"type": "text", "text": " Working"},
        ])
    );
    let buttons = blocks[3]["elements"].as_array().unwrap();
    assert_eq!(buttons[0]["url"], "https://x.slack.com/archives/D1/p1");
    assert_eq!(buttons[0]["text"]["text"], "Open thread");
    assert!(buttons[0].get("value").is_none(), "a link carries no token");
    assert_eq!(buttons[1]["value"], "hom:1:0:sel");
    assert_eq!(buttons[1]["text"]["text"], "Select");
    assert_eq!(buttons[2]["value"], "hom:1:0:stop");
    assert_eq!(buttons[2]["text"]["text"], "Stop");
    // No subject: two lines, the selected pane marked on its status line.
    assert_eq!(
        blocks[4]["elements"][0]["elements"],
        json!([
            {"type": "text", "text": "app/fix_bug (Codex)", "style": {"bold": true}},
            {"type": "text", "text": "\n"},
            {"type": "text", "text": "Working"},
            {"type": "text", "text": " (selected)", "style": {"italic": true}},
        ]),
        "no emoji without a status"
    );
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
    let blocks = page_blocks(&page);
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
            row.subject = Some("subject");
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
    let blocks = page_blocks(&page);
    let length = |value: &Value| value.as_str().unwrap().chars().count();
    assert_eq!(length(&blocks[0]["text"]["text"]), HEADER_LIMIT);
    assert_eq!(
        length(&blocks[2]["elements"][0]["elements"][0]["text"]),
        TEXT_LIMIT
    );
    assert_eq!(
        length(&blocks[3]["elements"][0]["text"]["text"]),
        BUTTON_LABEL_LIMIT
    );
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
