//! Bounding agent-authored text on its way out of the app.
//!
//! An agent writes its own session title and its own answers, and both travel
//! somewhere that did not author them — a phone message, another agent's tool
//! result. Every bound here is about that crossing: the text is unbounded and
//! untrusted at the source, so each consumer would otherwise have to defend
//! against it alone.
//!
//! One algorithm, two policies. A phone screen and an LLM context are not
//! bounded by the same thing, and a marker a person reads is localized while
//! one a model reads must not be — so [`elide_middle`] takes the budget and
//! the marker as arguments. The orchestrator's policy lives here beside it
//! (`bound_agent_text`, fixed English) because it has no other home; the
//! phone's lives at its call site in `remote_ops`, which owns the localized
//! string.
//!
//! Text flattened to one line also loses the invisible characters that can
//! reorder or hide words (bidi controls, zero-width spacers, tag characters):
//! rendered on a card, they could change what a person is asked to approve.
//!
//! GPUI-free.

/// A session title is agent-authored and unbounded. Capped at the one place a
/// `ChatSummary` is built, so the bound is a property of the type rather than
/// of one adapter's renderer.
const TITLE_MAX_CHARS: usize = 80;

/// Leading/trailing characters [`bound_agent_text`] keeps. Larger than the
/// phone's budget because the consumer is an LLM context, not a screen, and
/// the point is to avoid a second call to read the part that was cut.
const ORCHESTRATOR_HEAD_CHARS: usize = 2000;
const ORCHESTRATOR_TAIL_CHARS: usize = 2000;

/// How much of a prompt's start and end an approval card quotes. Enough to
/// recognize the request on a phone screen, and the end too, so an
/// instruction cannot hide behind a harmless opening. The card is one line.
pub const PROMPT_EXCERPT_HEAD_CHARS: usize = 80;
pub const PROMPT_EXCERPT_TAIL_CHARS: usize = 40;

/// Fixed English, like every other value in a tool result: the consumer is a
/// model branching on content, not a person reading their own locale.
const TRUNCATION_MARKER: &str = "[...truncated by daruda...]";

/// Flatten an agent-authored title to one bounded line, as [`one_line`]
/// does, so a multi-line title cannot break the row it is rendered on, and no
/// consumer has to defend against one.
pub fn sanitize_title(raw: &str) -> Option<String> {
    let clean: String = one_line(raw).chars().take(TITLE_MAX_CHARS).collect();
    (!clean.is_empty()).then_some(clean)
}

/// A title someone else wrote, as one line of a card: flattened as
/// [`one_line`] does and cut to the title bound with a trailing `…`.
pub fn title_line(raw: &str) -> String {
    excerpt_line(raw, TITLE_MAX_CHARS)
}

/// `raw` flattened as [`one_line`] does, cut to `max_chars` characters with
/// a trailing `…` when it is longer.
pub fn excerpt_line(raw: &str, max_chars: usize) -> String {
    let line = one_line(raw);
    match line.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

/// A prompt as an approval card quotes it: flattened as [`one_line`] does,
/// and past the bound its start and end around `elided(hidden_chars)`, the
/// caller's (localized) marker saying how much was left out.
pub fn prompt_excerpt(raw: &str, elided: impl FnOnce(usize) -> String) -> String {
    let line = one_line(raw);
    match split_ends(&line, PROMPT_EXCERPT_HEAD_CHARS, PROMPT_EXCERPT_TAIL_CHARS) {
        Some((head, hidden, tail)) => format!("{head}{}{tail}", elided(hidden)),
        None => line,
    }
}

/// Invisible characters are dropped, control characters become spaces, and
/// whitespace runs collapse to one space.
pub fn one_line(raw: &str) -> String {
    raw.chars()
        .filter(|c| !hides_or_reorders(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Bidi controls reorder the text after them; zero-width spaces, invisible
/// operators and the soft hyphen hide word breaks; tag characters carry text
/// no one sees but an agent reads. ZWNJ/ZWJ pass: they hide nothing, and
/// Persian, Indic and emoji sequences need them. Hangul fillers (U+115F,
/// U+1160, U+3164, U+FFA0) and blank Braille (U+2800) pass as letters that
/// show a gap at most and reorder nothing; dropping them corrupts real text.
fn hides_or_reorders(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{00AD}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{2060}'..='\u{2064}'
            | '\u{FEFF}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// `text`'s first `head` and last `tail` characters, and how many lie
/// between them; `None` while it fits in `head + tail`, so nothing is cut.
/// Counts `char`s, so neither end splits a character.
fn split_ends(text: &str, head: usize, tail: usize) -> Option<(&str, usize, &str)> {
    let (head_end, _) = text.char_indices().nth(head)?;
    let tail_start = if tail == 0 {
        text.len()
    } else {
        text.char_indices().rev().nth(tail - 1)?.0
    };
    if head_end >= tail_start {
        return None;
    }
    let hidden = text[head_end..tail_start].chars().count();
    Some((&text[..head_end], hidden, &text[tail_start..]))
}

/// Keep `text` verbatim while it fits in `head + tail` characters; past that,
/// keep the two ends around `marker`.
///
/// The threshold is `head + tail` rather than its own parameter: a threshold
/// below the sum would elide a middle that was never there, and one above it
/// would drop characters the caller asked to keep. There is one value that is
/// not a bug, so it is derived instead of passed.
///
/// Counts `char`s, not bytes, so a multi-byte response never splits
/// mid-character. Line breaks are preserved — a caller wanting one line wants
/// [`sanitize_title`].
pub fn elide_middle(text: &str, head: usize, tail: usize, marker: &str) -> String {
    match split_ends(text, head, tail) {
        Some((head, _, tail)) => format!("{head}\n{marker}\n{tail}"),
        None => text.to_string(),
    }
}

/// Bound an agent's answer for a tool result. `None` for text that carries
/// nothing — a caller must not hand a model an empty field and let it guess
/// whether that means silence or absence.
///
/// Deliberately does *not* flatten: this is markdown prose, and a body whose
/// line breaks were collapsed would arrive as one unreadable paragraph.
pub fn bound_agent_text(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    Some(elide_middle(
        raw,
        ORCHESTRATOR_HEAD_CHARS,
        ORCHESTRATOR_TAIL_CHARS,
        TRUNCATION_MARKER,
    ))
}
