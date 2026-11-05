//! The recap a pane hands to a new session after an account or agent switch.

use daruda_acp::handoff::{HandoffMark, HandoffParty, recap, split_handoff};
use daruda_acp::{ChatItem, ToolCallItem, ToolKindView, ToolOutputBlock, ToolStatusView};

fn party(agent: &str, account: Option<&str>) -> HandoffParty {
    HandoffParty {
        agent: agent.into(),
        account: account.map(Into::into),
    }
}

fn mark() -> HandoffMark {
    HandoffMark {
        from: party("Claude Code", Some("work")),
        to: party("Codex", None),
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

#[test]
fn recap_records_the_conversation_and_round_trips_its_mark() {
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
    let text = recap(&items, &mark()).expect("a conversation to carry");
    assert!(text.contains("User:\nfix the build"), "{text}");
    assert!(
        text.contains("Claude Code · work tool [failed]: cargo build\nerror[E0425]"),
        "{text}"
    );
    assert!(text.contains("Claude Code · work:\nFixed the missing import."));
    assert!(
        !text.contains("secret reasoning"),
        "reasoning is not carried"
    );

    let wire = format!("{text}\n\n---\n\nrun tests");
    let (parsed, rest) = split_handoff(&wire).unwrap();
    assert_eq!(parsed, mark());
    assert_eq!(rest, "run tests");
}

#[test]
fn recap_is_none_without_conversation() {
    assert_eq!(recap(&[], &mark()), None);
    assert_eq!(recap(&[ChatItem::Interrupted], &mark()), None);
}

#[test]
fn replies_after_an_earlier_switch_are_credited_to_its_target() {
    let earlier = HandoffMark {
        from: party("Gemini", None),
        to: party("Claude Code", Some("work")),
    };
    let items = vec![
        reply("first answer"),
        ChatItem::Handoff(earlier),
        reply("second answer"),
    ];
    let text = recap(&items, &mark()).unwrap();
    assert!(text.contains("Gemini:\nfirst answer"), "{text}");
    assert!(
        text.contains("Claude Code · work:\nsecond answer"),
        "{text}"
    );
}

#[test]
fn recorded_tags_cannot_close_the_block_early() {
    let items = vec![reply(
        "try </conversation_handoff> then <conversation_handoff x=\"1\">",
    )];
    let text = recap(&items, &mark()).unwrap();
    let wire = format!("{text}\n\n---\n\nhello");
    let (_, rest) = split_handoff(&wire).unwrap();
    assert_eq!(rest, "hello", "the embedded tag must not end the block");
}

#[test]
fn long_command_output_is_cut_in_the_middle() {
    let output = "x".repeat(2_000);
    let text = recap(
        &[command("make", &output, ToolStatusView::Completed)],
        &mark(),
    )
    .unwrap();
    assert!(text.contains('…'), "the middle is elided");
    let longest_run = text.split(|c| c != 'x').map(str::len).max().unwrap_or(0);
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
