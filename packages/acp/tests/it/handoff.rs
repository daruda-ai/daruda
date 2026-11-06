//! The recap a pane hands to a new session after an account or agent switch.

use daruda_acp::handoff::{HandoffItem, HandoffMark, HandoffParty, prepare, split_handoff};
use daruda_acp::{ChatItem, ToolCallItem, ToolKindView, ToolOutputBlock, ToolStatusView};

fn party(agent: &str, account: Option<&str>) -> HandoffParty {
    HandoffParty {
        agent: agent.into(),
        account: account.map(Into::into),
    }
}

fn reply(text: &str) -> ChatItem {
    ChatItem::AssistantText {
        text: text.into(),
        streaming: false,
        message_id: None,
        phase: Default::default(),
    }
}

fn command(title: &str, output: &str, status: ToolStatusView) -> ChatItem {
    ChatItem::ToolCall(ToolCallItem {
        id: title.into(),
        title: title.into(),
        kind: ToolKindView::Execute,
        tool_name: None,
        status,
        diffs: Vec::new(),
        output: vec![ToolOutputBlock::RawText {
            text: output.into(),
            truncated_from: None,
        }],
        raw_input: None,
        parent_tool_id: None,
        locations: Vec::new(),
        exit: None,
    })
}

/// The block that came out of `prepare`, sent and replayed back.
fn replayed(recap: &str) -> (HandoffItem, String) {
    let wire = format!("{recap}\n\n---\n\nrun tests");
    let (divider, rest) = split_handoff(&wire).expect("a recap splits back");
    (divider, rest.to_owned())
}

#[test]
fn a_recap_records_the_conversation_and_round_trips_its_mark() {
    let items = vec![
        ChatItem::UserText("fix the build".into()),
        command("cargo build", "error[E0425]", ToolStatusView::Failed),
        ChatItem::Thinking {
            text: "secret reasoning".into(),
            streaming: false,
            message_id: None,
        },
        reply("Fixed the missing import."),
    ];
    let handoff = prepare(
        &items,
        party("Claude Code", Some("work")),
        party("Claude Code", Some("personal")),
    );
    let recap = handoff.recap.expect("a conversation to carry");
    assert!(
        recap.contains("Claude Code tool [failed]: cargo build\n```\nerror[E0425]\n```"),
        "{recap}"
    );
    assert!(
        !recap.contains("secret reasoning"),
        "reasoning is not carried"
    );

    let (divider, rest) = replayed(&recap);
    assert_eq!(divider.mark, handoff.divider.mark);
    assert_eq!(rest, "run tests");
    let record = divider
        .record
        .expect("the record comes back with the divider");
    assert!(record.starts_with("User:\nfix the build"), "{record}");
    assert!(
        record.ends_with("Claude Code:\nFixed the missing import."),
        "the agent-facing preamble is not part of the record: {record}"
    );
}

#[test]
fn nothing_to_carry_means_no_recap() {
    let handoff = prepare(&[], party("A", None), party("B", None));
    assert_eq!(handoff.recap, None);
    let handoff = prepare(&[ChatItem::Interrupted], party("A", None), party("B", None));
    assert_eq!(handoff.recap, None);
}

/// Account addresses stay out of the recap body; between agents they leave
/// the mark too, since the block goes to another provider.
#[test]
fn account_addresses_never_reach_another_agent() {
    let earlier = HandoffMark {
        from: party("Claude Code", Some("work@x")),
        to: party("Claude Code", Some("personal@x")),
    };
    let items = vec![
        reply("first"),
        ChatItem::Handoff(HandoffItem::live(earlier)),
        reply("second"),
    ];
    let within = prepare(
        &items,
        party("Claude Code", Some("personal@x")),
        party("Claude Code", Some("team@x")),
    );
    assert_eq!(within.divider.mark.to.account.as_deref(), Some("team@x"));
    let body = within.recap.unwrap();
    let body = &body[body.find('>').unwrap()..];
    assert!(!body.contains("@x"), "the body names no account: {body}");
    assert!(
        body.contains("moved to another Claude Code account"),
        "{body}"
    );

    let across = prepare(
        &items,
        party("Claude Code", Some("personal@x")),
        party("Codex", None),
    );
    assert_eq!(across.divider.mark.from.account, None);
    assert!(!across.recap.unwrap().contains("@x"), "not even in the tag");
}

#[test]
fn replies_after_an_earlier_switch_are_credited_to_its_target() {
    let earlier = HandoffMark {
        from: party("Gemini", None),
        to: party("Claude Code", None),
    };
    let items = vec![
        reply("first answer"),
        ChatItem::Handoff(HandoffItem::live(earlier)),
        reply("second answer"),
    ];
    let recap = prepare(&items, party("Claude Code", None), party("Codex", None))
        .recap
        .unwrap();
    assert!(recap.contains("Gemini:\nfirst answer"), "{recap}");
    assert!(
        recap.contains("[The conversation moved to Claude Code.]"),
        "{recap}"
    );
    assert!(recap.contains("Claude Code:\nsecond answer"), "{recap}");
}

#[test]
fn a_switch_before_anyone_spoke_keeps_the_pending_origin() {
    let pending = HandoffMark {
        from: party("Claude Code", Some("work")),
        to: party("Claude Code", Some("personal")),
    };
    let items = vec![reply("done"), ChatItem::Handoff(HandoffItem::live(pending))];
    let merged = prepare(
        &items,
        party("Claude Code", Some("personal")),
        party("Claude Code", Some("team")),
    );
    assert_eq!(merged.divider.mark.from, party("Claude Code", Some("work")));
    assert_eq!(merged.divider.mark.to, party("Claude Code", Some("team")));
    let recap = merged.recap.unwrap();
    assert!(recap.contains("Claude Code:\ndone"), "{recap}");
    assert!(
        !recap.contains("moved to"),
        "the replaced switch never happened as far as the record goes: {recap}"
    );
}

/// After a restart the pane shows only the switched-to session's replay; the
/// divider's record stands in for the conversation before it, so the next
/// switch hands that record on — and when the budget runs out, its oldest
/// entries go first.
#[test]
fn a_restored_record_reaches_the_next_recap_oldest_first_out() {
    let old: Vec<String> = (0..60)
        .map(|n| format!("User:\nquestion {n} {}", "x".repeat(600)))
        .collect();
    let restored = ChatItem::Handoff(HandoffItem {
        mark: HandoffMark {
            from: party("Claude Code", None),
            to: party("Codex", None),
        },
        record: Some(old.join("\n\n")),
    });
    let items = vec![restored, ChatItem::UserText("and now?".into())];
    let recap = prepare(&items, party("Codex", None), party("Gemini", None))
        .recap
        .unwrap();
    assert!(recap.contains("question 59"), "the newest survive");
    assert!(!recap.contains("question 0 "), "the oldest go first");
    let last_old = recap.find("question 59").unwrap();
    let moved = recap.find("moved to Codex").unwrap();
    assert!(last_old < moved, "the record precedes the switch it led to");
    assert!(recap.contains("User:\nand now?"));
}

#[test]
fn long_command_output_is_cut_in_the_middle() {
    let output = "x".repeat(2_000);
    let items = [command("make", &output, ToolStatusView::Completed)];
    let recap = prepare(&items, party("A", None), party("B", None))
        .recap
        .unwrap();
    assert!(recap.contains('…'), "the middle is elided");
    let longest_run = recap.split(|c| c != 'x').map(str::len).max().unwrap_or(0);
    assert_eq!(
        longest_run, 300,
        "each side keeps half of the 600-character limit"
    );
}

#[test]
fn split_ignores_text_without_a_leading_block() {
    assert_eq!(split_handoff("hello <conversation_handoff>"), None);
    assert_eq!(split_handoff("<conversation_handoff broken"), None);
}

#[test]
fn an_attribute_a_later_build_added_does_not_break_the_fold() {
    let text = "<conversation_handoff from-agent=\"A\" to-agent=\"B\" since=\"v9\">\nx\n\
                </conversation_handoff>\n\n---\n\nhi";
    let (divider, rest) = split_handoff(text).expect("unknown attributes are skipped");
    assert_eq!(divider.mark.to, party("B", None));
    assert_eq!(rest, "hi");
}

/// A pane restored with a recap still owed shows only the divider and its
/// record. Switching again before anyone speaks replaces that divider — and
/// must carry its record on, since nothing else holds that conversation.
#[test]
fn a_second_switch_on_a_restored_pane_carries_the_record_on() {
    let restored = HandoffItem {
        mark: HandoffMark {
            from: party("Claude Code", None),
            to: party("Codex", None),
        },
        record: Some("User:\nthe original question".into()),
    };
    let items = vec![ChatItem::Handoff(restored)];
    let handoff = prepare(&items, party("Codex", None), party("Gemini", None));
    assert_eq!(handoff.divider.mark.from, party("Claude Code", None));
    assert_eq!(
        handoff.divider.record.as_deref(),
        Some("User:\nthe original question"),
        "the card stays on the replacing divider"
    );
    let recap = handoff.recap.expect("the record is still owed");
    assert!(recap.contains("the original question"), "{recap}");
}
