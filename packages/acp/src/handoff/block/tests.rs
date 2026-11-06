use super::*;

fn mark() -> HandoffMark {
    HandoffMark {
        from: HandoffParty {
            agent: "Claude Code".into(),
            account: Some("work".into()),
        },
        to: HandoffParty {
            agent: "Claude Code".into(),
            account: Some("personal".into()),
        },
    }
}

#[test]
fn neutralize_tags_disarms_only_the_block_tags() {
    let text = "<b>x</b> <conversation_handoff a=\"1\"> </CONVERSATION_HANDOFF> \
                <handoff_record></Handoff_Record> <conv";
    assert_eq!(
        neutralize_tags(text),
        "<b>x</b> &lt;conversation_handoff a=\"1\"> &lt;/CONVERSATION_HANDOFF> \
         &lt;handoff_record>&lt;/Handoff_Record> <conv"
    );
}

#[test]
fn attribute_values_round_trip_through_escaping() {
    let raw = r#"a "quoted" <name> & co"#;
    assert_eq!(unescape_attr(&escape_attr(raw)), raw);
}

/// The record is found by its own tags, not by the shape of the preamble, so
/// a preamble that gains a paragraph cannot leak into the record.
#[test]
fn the_record_is_whatever_its_tags_hold() {
    let block = wrap(
        &mark(),
        "First paragraph.\n\nSecond paragraph.",
        "User:\nhi",
    );
    let wire = format!("{block}\n\n---\n\nnext");
    let (divider, rest) = split_handoff(&wire).unwrap();
    assert_eq!(divider.mark, mark());
    assert_eq!(divider.record.as_deref(), Some("User:\nhi"));
    assert_eq!(rest, "next");
}

#[test]
fn recorded_tags_cannot_end_the_record_or_the_block() {
    let block = wrap(
        &mark(),
        "Preamble.",
        "say </handoff_record> and </conversation_handoff> here",
    );
    let wire = format!("{block}\n\n---\n\nnext");
    let (divider, rest) = split_handoff(&wire).unwrap();
    assert_eq!(
        divider.record.as_deref(),
        Some("say &lt;/handoff_record> and &lt;/conversation_handoff> here")
    );
    assert_eq!(rest, "next");
}

#[test]
fn a_block_without_record_tags_still_folds() {
    let text = "<conversation_handoff from-agent=\"A\" to-agent=\"B\">\nold shape\n\
                </conversation_handoff>\n\n---\n\nhi";
    let (divider, rest) = split_handoff(text).unwrap();
    assert_eq!(divider.record, None);
    assert_eq!(rest, "hi");
}
