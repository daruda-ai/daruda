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
fn keep_newest_drops_the_oldest_entries_first() {
    let entries = vec![
        Entry::Turn("old".repeat(10)),
        Entry::Turn("mid".repeat(10)),
        Entry::Turn("new".repeat(10)),
    ];
    // Each entry is 30 characters; 90 fits two plus their separators.
    let kept = keep_newest(entries, 90);
    assert!(
        kept.starts_with("[… 1 earlier entries omitted …]"),
        "{kept}"
    );
    assert!(kept.contains(&"mid".repeat(10)));
    assert!(kept.ends_with(&"new".repeat(10)));
}

#[test]
fn a_fence_outgrows_the_backticks_it_holds() {
    assert_eq!(fenced("plain"), "```\nplain\n```");
    assert_eq!(fenced("a ```` b"), "`````\na ```` b\n`````");
}

#[test]
fn a_record_over_budget_keeps_its_newest_turns() {
    let record = ["User:\nfirst", "Codex:\nsecond", "User:\nthird"].join("\n\n");
    let kept = keep_newest(vec![Entry::Record(record)], 30);
    assert_eq!(
        kept,
        "[… earlier entries omitted …]\n\nCodex:\nsecond\n\nUser:\nthird"
    );
}

/// A cut that would start inside a code fence moves past it instead: the
/// fence's closing line alone would make everything after it code.
#[test]
fn a_record_is_never_cut_inside_a_fence() {
    let record = [
        "Codex tool: make\n```\nline one",
        "line two after a blank line\n```",
        "User:\nnext",
    ]
    .join("\n\n");
    // Room for the last two parts, which would open with the fence's tail.
    let tail = record_tail(&record, 60).expect("something fits");
    assert_eq!(tail, "User:\nnext");
    assert!(fences_balanced(&tail));
}

#[test]
fn a_longer_fence_holds_shorter_ones() {
    assert!(fences_balanced("````\n```\ninner\n```\n````"));
    assert!(!fences_balanced("```\nopen"));
}
