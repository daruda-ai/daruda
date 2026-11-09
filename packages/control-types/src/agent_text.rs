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
//! Text flattened to one line also loses Unicode format characters (bidi
//! overrides, zero-width marks): rendered on a card, they could reorder or
//! hide the words a person is asked to approve.
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

/// The most of a prompt an approval card quotes. Enough to recognize the
/// request on a phone screen; the card is one line, and the full text is the
/// agent's.
pub const PROMPT_EXCERPT_CHARS: usize = 120;

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

/// `raw` flattened as [`one_line`] does, cut to `max_chars` characters with
/// a trailing `…` when it is longer.
pub fn excerpt_line(raw: &str, max_chars: usize) -> String {
    let line = one_line(raw);
    match line.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

/// The start of a prompt as an approval card quotes it.
pub fn prompt_excerpt(raw: &str) -> String {
    excerpt_line(raw, PROMPT_EXCERPT_CHARS)
}

/// Format characters are dropped, control characters become spaces, and
/// whitespace runs collapse to one space.
fn one_line(raw: &str) -> String {
    raw.chars()
        .filter(|c| !is_format_char(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Unicode general category `Cf`. std has no category lookup, and the set is
/// small and stable, so it is listed rather than pulled in as a dependency.
fn is_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
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
    let Some((head_end, _)) = text.char_indices().nth(head) else {
        return text.to_string();
    };
    let tail_start = if tail == 0 {
        text.len()
    } else {
        let Some((start, _)) = text.char_indices().rev().nth(tail - 1) else {
            return text.to_string();
        };
        start
    };
    if head_end >= tail_start {
        return text.to_string();
    }
    let head = &text[..head_end];
    let tail = &text[tail_start..];
    format!("{head}\n{marker}\n{tail}")
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
