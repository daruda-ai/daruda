use super::*;

#[test]
fn truncate_middle_keeps_both_ends_of_a_long_text() {
    let text = "a".repeat(10) + &"b".repeat(10);
    let cut = truncate_middle(&text, 10);
    assert_eq!(cut, "aaaaa\n…\nbbbbb");
    assert_eq!(truncate_middle("short", 10), "short");
}

#[test]
fn truncate_middle_counts_characters_not_bytes() {
    let text = "가".repeat(12);
    assert_eq!(truncate_middle(&text, 4), "가가\n…\n가가");
}

#[test]
fn neutralize_tags_disarms_only_the_block_tags() {
    let text = "<b>x</b> <conversation_handoff a=\"1\"> </CONVERSATION_HANDOFF> <conv";
    assert_eq!(
        neutralize_tags(text),
        "<b>x</b> &lt;conversation_handoff a=\"1\"> &lt;/CONVERSATION_HANDOFF> <conv"
    );
}

#[test]
fn attribute_values_round_trip_through_escaping() {
    let raw = r#"a "quoted" <name> & co"#;
    assert_eq!(unescape_attr(&escape_attr(raw)), raw);
}

#[test]
fn keep_newest_drops_the_oldest_entries_first() {
    let entries = vec!["old".repeat(10), "mid".repeat(10), "new".repeat(10)];
    // Each entry is 30 characters; 90 fits two plus their separators.
    let kept = keep_newest(entries, 90);
    assert!(
        kept.starts_with("[… 1 earlier entries omitted …]"),
        "{kept}"
    );
    assert!(kept.contains(&"mid".repeat(10)));
    assert!(kept.ends_with(&"new".repeat(10)));
}
