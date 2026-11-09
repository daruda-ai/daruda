use daruda_control_types::agent_text::{
    PROMPT_EXCERPT_HEAD_CHARS, PROMPT_EXCERPT_TAIL_CHARS, bound_agent_text, elide_middle,
    excerpt_line, one_line, prompt_excerpt, sanitize_title, title_line,
};

/// The marker a test reads back: the hidden count, nothing localized.
fn marker(hidden: usize) -> String {
    format!(" [+{hidden}] ")
}

#[test]
fn elide_middle_keeps_short_text_verbatim() {
    assert_eq!(elide_middle("abc", 2, 2, "…"), "abc");
    // Exactly at the budget is still verbatim: there is no middle to cut.
    assert_eq!(elide_middle("abcd", 2, 2, "…"), "abcd");
}

#[test]
fn elide_middle_keeps_both_ends_around_the_marker() {
    assert_eq!(elide_middle("abcdef", 2, 2, "CUT"), "ab\nCUT\nef");
}

/// A byte-indexed implementation panics or splits a character here.
#[test]
fn elide_middle_counts_chars_not_bytes() {
    let text = "가나다라마바";
    let out = elide_middle(text, 2, 2, "CUT");
    assert_eq!(out, "가나\nCUT\n마바");
    // Every retained char survived whole.
    assert!(out.chars().all(|c| c == '\n' || "가나마바CUT".contains(c)));
}

#[test]
fn elision_accepts_empty_sides_and_oversized_budgets() {
    for (head, tail, expected) in [
        (0, 2, "\nCUT\né😀"),
        (2, 0, "한e\nCUT\n"),
        (0, 0, "\nCUT\n"),
        (usize::MAX, 1, "한eé😀"),
        (1, usize::MAX, "한eé😀"),
        (usize::MAX, usize::MAX, "한eé😀"),
    ] {
        assert_eq!(elide_middle("한eé😀", head, tail, "CUT"), expected);
    }
    assert_eq!(elide_middle("", 0, 0, "CUT"), "");
}

#[test]
fn a_bounded_body_carries_its_marker_in_band() {
    let long = "x".repeat(4001);
    let out = bound_agent_text(&long).expect("non-empty");
    assert!(
        out.contains("[...truncated by daruda...]"),
        "truncation must be self-describing, not a sibling bool: {out:.80}"
    );
    assert_eq!(out.chars().filter(|c| *c == 'x').count(), 4000);
}

#[test]
fn a_body_under_the_budget_is_untouched() {
    let body = "line one\n\nline two";
    assert_eq!(bound_agent_text(body).as_deref(), Some(body));
}

/// The difference from [`sanitize_title`]: prose keeps its shape.
#[test]
fn a_bounded_body_keeps_its_line_breaks() {
    let body = "## heading\n\n- one\n- two";
    let out = bound_agent_text(body).expect("non-empty");
    assert_eq!(out.matches('\n').count(), body.matches('\n').count());
}

#[test]
fn a_blank_body_is_absent() {
    assert_eq!(bound_agent_text(""), None);
    assert_eq!(bound_agent_text("   \n\t  "), None);
}

#[test]
fn a_title_is_capped_and_stripped_of_control_characters() {
    let raw = format!("line one\nline two\t{}", "x".repeat(200));
    let clean = sanitize_title(&raw).expect("non-empty");
    assert_eq!(clean.chars().count(), 80);
    assert!(!clean.contains('\n') && !clean.contains('\t'));
}

#[test]
fn a_short_title_is_untouched_and_a_blank_one_is_absent() {
    assert_eq!(
        sanitize_title("restore invariants").as_deref(),
        Some("restore invariants")
    );
    assert_eq!(sanitize_title("   \n\t  "), None);
    assert_eq!(sanitize_title(""), None);
}

#[test]
fn a_short_prompt_is_quoted_whole_on_one_line() {
    assert_eq!(
        prompt_excerpt("fix\n\tthe  login\r\nbug", marker),
        "fix the login bug"
    );
}

/// A harmless opening cannot hide the instruction after it: past the bound
/// the card quotes both ends and says how much it left out.
#[test]
fn a_long_prompt_keeps_its_start_and_end_around_the_hidden_count() {
    let budget = PROMPT_EXCERPT_HEAD_CHARS + PROMPT_EXCERPT_TAIL_CHARS;
    let exact = "a".repeat(budget);
    assert_eq!(prompt_excerpt(&exact, marker), exact);

    let prompt = format!(
        "{}{}{}",
        "가".repeat(PROMPT_EXCERPT_HEAD_CHARS),
        "x".repeat(7),
        "😀".repeat(PROMPT_EXCERPT_TAIL_CHARS)
    );
    assert_eq!(
        prompt_excerpt(&prompt, marker),
        format!(
            "{}{}{}",
            "가".repeat(PROMPT_EXCERPT_HEAD_CHARS),
            marker(7),
            "😀".repeat(PROMPT_EXCERPT_TAIL_CHARS)
        )
    );
    // The hidden count is of the flattened line, not the raw text.
    let spaced = format!("{}\n\n\n{}", "a".repeat(budget), "b");
    assert_eq!(
        prompt_excerpt(&spaced, marker),
        format!(
            "{}{}{} b",
            "a".repeat(PROMPT_EXCERPT_HEAD_CHARS),
            marker(2),
            "a".repeat(PROMPT_EXCERPT_TAIL_CHARS - 2)
        )
    );
}

#[test]
fn an_excerpt_is_cut_with_an_ellipsis_and_a_title_at_its_bound() {
    assert_eq!(excerpt_line("abcdef", 3), "abc…");
    assert_eq!(excerpt_line("abc", 3), "abc");
    let title = title_line(&"t".repeat(200));
    assert_eq!(title.chars().count(), 81);
    assert!(title.ends_with('…'));
    assert_eq!(title_line("Fix\nthe login"), "Fix the login");
}

/// A right-to-left override would show `txt.exe` as `exe.txt`; zero-width
/// and bidi marks could hide or reorder the words a card asks about; tag
/// characters carry text nobody sees.
#[test]
fn reordering_and_hiding_characters_are_dropped_from_excerpts_and_titles() {
    let hostile =
        "run \u{202E}exe.txt\u{202C} now\u{200B}\u{2066}!\u{2069}\u{FEFF}\u{061C}\u{200E}\u{00AD}";
    assert_eq!(excerpt_line(hostile, 80), "run exe.txt now!");
    assert_eq!(sanitize_title(hostile).as_deref(), Some("run exe.txt now!"));
    assert_eq!(title_line("a\u{202E}b"), "ab");
    assert_eq!(sanitize_title("\u{200B}\u{202E}"), None);
    assert_eq!(one_line("ok\u{E0041}\u{E0042}"), "ok");
}

/// Joiners shape Persian, Indic scripts and emoji sequences, and cannot
/// reorder or hide a word, so they survive.
#[test]
fn joiners_survive_so_scripts_and_emoji_keep_their_shape() {
    let family = "👩\u{200D}👩\u{200D}👧";
    assert_eq!(one_line(family), family);
    let persian = "می\u{200C}خواهم";
    assert_eq!(one_line(persian), persian);
    assert_eq!(prompt_excerpt(family, marker), family);
}
