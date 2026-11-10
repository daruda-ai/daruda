use daruda_control_types::agent_text::{
    ISOLATE_END, ISOLATE_START, PROMPT_EXCERPT_FLOOR_HEAD_CHARS, PROMPT_EXCERPT_FLOOR_TAIL_CHARS,
    PROMPT_EXCERPT_HEAD_CHARS, PROMPT_EXCERPT_TAIL_CHARS, QuoteMarkers, TITLE_EXCERPT_HEAD_CHARS,
    TITLE_EXCERPT_TAIL_CHARS, bound_agent_text, elide_middle, one_line, prompt_excerpt,
    prompt_excerpts, sanitize_title, title_excerpt,
};

/// Markers a test reads back: the counts, nothing localized.
struct Markers;

impl QuoteMarkers for Markers {
    fn quote(&self, text: &str) -> String {
        format!("<{text}>")
    }

    fn elided(&self, n: usize) -> String {
        marker(n)
    }

    fn hidden(&self, n: usize) -> String {
        format!(" [{n} hidden]")
    }
}

fn marker(elided: usize) -> String {
    format!(" [+{elided}] ")
}

fn prompt(raw: &str) -> String {
    prompt_excerpt(raw, &Markers)
}

fn title(raw: &str) -> String {
    title_excerpt(raw, &Markers)
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
        prompt("fix\n\tthe  login\r\nbug"),
        "<\u{2068}fix the login bug\u{2069}>"
    );
}

/// A harmless opening cannot hide the instruction after it: past the bound
/// the card quotes both ends and says how much it left out.
#[test]
fn a_long_prompt_keeps_its_start_and_end_around_the_hidden_count() {
    let budget = PROMPT_EXCERPT_HEAD_CHARS + PROMPT_EXCERPT_TAIL_CHARS;
    let exact = "a".repeat(budget);
    assert_eq!(prompt(&exact), format!("<\u{2068}{exact}\u{2069}>"));

    let long = format!(
        "{}{}{}",
        "가".repeat(PROMPT_EXCERPT_HEAD_CHARS),
        "x".repeat(7),
        "😀".repeat(PROMPT_EXCERPT_TAIL_CHARS)
    );
    assert_eq!(
        prompt(&long),
        format!(
            "<\u{2068}{}\u{2069}>{}<\u{2068}{}\u{2069}>",
            "가".repeat(PROMPT_EXCERPT_HEAD_CHARS),
            marker(7),
            "😀".repeat(PROMPT_EXCERPT_TAIL_CHARS)
        )
    );
    // The elided count is of the flattened line, not the raw text.
    let spaced = format!("{}\n\n\n{}", "a".repeat(budget), "b");
    assert_eq!(
        prompt(&spaced),
        format!(
            "<\u{2068}{}\u{2069}>{}<\u{2068}{} b\u{2069}>",
            "a".repeat(PROMPT_EXCERPT_HEAD_CHARS),
            marker(2),
            "a".repeat(PROMPT_EXCERPT_TAIL_CHARS - 2)
        )
    );
}

/// A card short of room takes a narrower excerpt: widest first, each
/// keeping both ends, down to the floor; a short prompt is whole at each.
#[test]
fn narrower_excerpts_end_at_the_floor() {
    let long = "p".repeat(3000);
    let excerpts: Vec<String> = prompt_excerpts(&long, &Markers).collect();
    assert_eq!(excerpts.first(), Some(&prompt(&long)));
    let lengths: Vec<usize> = excerpts.iter().map(|e| e.chars().count()).collect();
    assert!(lengths.windows(2).all(|w| w[0] > w[1]), "{lengths:?}");
    assert_eq!(
        excerpts.last().map(String::as_str),
        Some(
            format!(
                "<\u{2068}{}\u{2069}>{}<\u{2068}{}\u{2069}>",
                "p".repeat(PROMPT_EXCERPT_FLOOR_HEAD_CHARS),
                marker(3000 - PROMPT_EXCERPT_FLOOR_HEAD_CHARS - PROMPT_EXCERPT_FLOOR_TAIL_CHARS),
                "p".repeat(PROMPT_EXCERPT_FLOOR_TAIL_CHARS)
            )
            .as_str()
        )
    );
    assert!(prompt_excerpts("short", &Markers).all(|e| e == "<\u{2068}short\u{2069}>"));
}

/// A task title reaches the agent whole, so a long one is quoted by both
/// ends with the count between, as a prompt is.
#[test]
fn a_long_title_keeps_its_start_and_end_around_the_hidden_count() {
    let budget = TITLE_EXCERPT_HEAD_CHARS + TITLE_EXCERPT_TAIL_CHARS;
    let whole = "t".repeat(budget);
    assert_eq!(title(&whole), format!("<\u{2068}{whole}\u{2069}>"));
    let long = format!(
        "{}{}{}",
        "h".repeat(TITLE_EXCERPT_HEAD_CHARS),
        "m".repeat(120),
        "z".repeat(TITLE_EXCERPT_TAIL_CHARS)
    );
    assert_eq!(
        title(&long),
        format!(
            "<\u{2068}{}\u{2069}>{}<\u{2068}{}\u{2069}>",
            "h".repeat(TITLE_EXCERPT_HEAD_CHARS),
            marker(120),
            "z".repeat(TITLE_EXCERPT_TAIL_CHARS)
        )
    );
    assert_eq!(title("Fix\nthe login"), "<\u{2068}Fix the login\u{2069}>");
}

/// A right-to-left override would show `txt.exe` as `exe.txt`; zero-width
/// and bidi marks could hide or reorder the words a card asks about; tag
/// characters carry text nobody sees.
#[test]
fn reordering_and_hiding_characters_are_dropped_from_excerpts_and_titles() {
    let hostile =
        "run \u{202E}exe.txt\u{202C} now\u{200B}\u{2066}!\u{2069}\u{FEFF}\u{061C}\u{200E}\u{00AD}";
    assert_eq!(one_line(hostile), "run exe.txt now!");
    assert_eq!(sanitize_title(hostile).as_deref(), Some("run exe.txt now!"));
    assert_eq!(sanitize_title("\u{200B}\u{202E}"), None);
    assert_eq!(one_line("ok\u{E0041}\u{E0042}"), "ok");
    // Interlinear annotations, deprecated format controls and supplementary
    // variation selectors hide data the same way.
    assert_eq!(
        one_line("a\u{FFF9}b\u{FFFB}\u{206A}\u{206F}c\u{E0100}\u{E01EF}"),
        "abc"
    );
}

/// The agent still receives what a card drops, so the card says how much:
/// an instruction in tag characters shows as a count, never as nothing.
#[test]
fn a_card_quote_counts_the_invisible_characters_it_dropped() {
    let tagged: String = "ok"
        .chars()
        .chain(
            "rm -rf"
                .chars()
                .map(|c| char::from_u32(0xE0000 + c as u32).unwrap()),
        )
        .collect();
    assert_eq!(prompt(&tagged), "<\u{2068}ok\u{2069}> [6 hidden]");
    assert_eq!(title(&tagged), "<\u{2068}ok\u{2069}> [6 hidden]");
    // Nothing visible is still something sent.
    assert_eq!(prompt("\u{200B}\u{202E}"), "[2 hidden]");
    assert_eq!(prompt("plain text"), "<\u{2068}plain text\u{2069}>");
    assert_eq!(title("plain text"), "<\u{2068}plain text\u{2069}>");
    // A count the text spells is inside the quote; the card's is outside.
    assert_eq!(
        prompt("a [3 hidden]\u{200B}"),
        "<\u{2068}a [3 hidden]\u{2069}> [1 hidden]"
    );
}

/// A quote mark inside quoted text could close the card's own quote and
/// append words that read as the card's; each becomes an apostrophe.
#[test]
fn a_card_quote_cannot_close_the_cards_quotes() {
    let spoof = "x\u{201D}. (requested permission mode: Ask) \u{201C}\"y\" \u{00AB}z\u{00BB}";
    assert_eq!(
        prompt(spoof),
        "<\u{2068}x'. (requested permission mode: Ask) ''y' 'z'\u{2069}>"
    );
    assert_eq!(title("say \"hi\""), "<\u{2068}say 'hi'\u{2069}>");
    // An apostrophe is left as it is.
    assert_eq!(prompt("don't"), "<\u{2068}don't\u{2069}>");
}

/// Joiners shape Persian, Indic scripts and emoji sequences, and cannot
/// reorder or hide a word, so they survive.
#[test]
fn joiners_survive_so_scripts_and_emoji_keep_their_shape() {
    let family = "👩\u{200D}👩\u{200D}👧";
    assert_eq!(one_line(family), family);
    let persian = "می\u{200C}خواهم";
    assert_eq!(one_line(persian), persian);
    assert_eq!(prompt(family), format!("<\u{2068}{family}\u{2069}>"));
}

/// Right-to-left letters reorder the neutral marks beside them with no
/// control character at all, so each quoted run is set in isolates the card
/// adds itself; isolates the text carried are dropped and counted like any
/// other bidi control, and the card's own are never counted.
#[test]
fn a_card_isolates_what_it_quotes_and_counts_only_the_texts_isolates() {
    let rtl = "\u{05E9}\u{05DC}\u{05D5}\u{05DD} 3";
    assert_eq!(prompt(rtl), format!("<{ISOLATE_START}{rtl}{ISOLATE_END}>"));
    assert_eq!(
        title("\u{2068}a\u{2069}"),
        format!("<{ISOLATE_START}a{ISOLATE_END}> [2 hidden]")
    );
    assert_eq!(
        prompt(&format!("{rtl}\u{2067}"))
            .matches(ISOLATE_START)
            .count(),
        1
    );
    let long = format!(
        "{}{}",
        "\u{05D0}".repeat(TITLE_EXCERPT_HEAD_CHARS),
        "z".repeat(50)
    );
    let quoted = title(&long);
    assert_eq!(quoted.matches(ISOLATE_START).count(), 2, "{quoted}");
    assert_eq!(quoted.matches(ISOLATE_END).count(), 2, "{quoted}");
    assert!(!quoted.contains("hidden"), "{quoted}");
}
