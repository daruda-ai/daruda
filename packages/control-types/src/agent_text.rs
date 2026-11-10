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
//! The agent still receives them, so a card says how many it dropped. Which
//! characters those are, and which invisible ones pass uncounted because they
//! only modify the glyph beside them, is `daruda_core::text::hides_or_reorders`
//! — the same predicate a branch name is checked against. What a card then
//! quotes it sets in Unicode isolates of its own, so right-to-left text
//! cannot visually reorder the card's quote marks and counts around it.
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

/// How much of a prompt's start and end an approval card quotes at most:
/// about 1500 characters, enough to read a typical delegated prompt whole on
/// a phone, and the end too, so an instruction cannot hide behind a harmless
/// opening. The head is the larger share because it carries the request;
/// the tail is where an appended instruction would sit. The card is one line.
pub const PROMPT_EXCERPT_HEAD_CHARS: usize = 1000;
pub const PROMPT_EXCERPT_TAIL_CHARS: usize = 500;

/// The narrowest bound [`prompt_excerpts`] offers: what a card still quotes
/// when its other fields leave little room in one phone message.
pub const PROMPT_EXCERPT_FLOOR_HEAD_CHARS: usize = 80;
pub const PROMPT_EXCERPT_FLOOR_TAIL_CHARS: usize = 40;

/// Every bound [`prompt_excerpts`] tries, widest first, each keeping the
/// head's larger share.
const PROMPT_EXCERPT_BOUNDS: [(usize, usize); 4] = [
    (PROMPT_EXCERPT_HEAD_CHARS, PROMPT_EXCERPT_TAIL_CHARS),
    (500, 250),
    (240, 120),
    (
        PROMPT_EXCERPT_FLOOR_HEAD_CHARS,
        PROMPT_EXCERPT_FLOOR_TAIL_CHARS,
    ),
];

/// The same for a task title, which the agent receives whole: one up to
/// `head + tail` characters is quoted whole, a longer one by both ends.
/// Unrelated to `TITLE_MAX_CHARS`, which caps a session's own title.
pub const TITLE_EXCERPT_HEAD_CHARS: usize = 60;
pub const TITLE_EXCERPT_TAIL_CHARS: usize = 20;

/// Fixed English, like every other value in a tool result: the consumer is a
/// model branching on content, not a person reading their own locale.
const TRUNCATION_MARKER: &str = "[...truncated by daruda...]";

/// Quote characters a card's own framing uses; inside quoted text each
/// becomes an apostrophe, so the text cannot close the quote and append
/// words that read as the card's.
const FRAMING_QUOTES: &[char] = &[
    '"', '\u{201C}', '\u{201D}', '\u{201E}', '\u{201F}', '\u{2033}', '\u{2036}', '\u{00AB}',
    '\u{00BB}', '\u{301D}', '\u{301E}', '\u{301F}', '\u{FF02}',
];
const NEUTRAL_QUOTE: char = '\'';

/// FIRST STRONG ISOLATE and POP DIRECTIONAL ISOLATE: what a card puts around
/// each text it quotes. Right-to-left letters need no control character to
/// reorder the neutral marks beside them — a card's own quote marks and
/// counts — so the quoted text is set apart as one unit, in its own
/// direction. Added after flattening, which drops and counts any isolate the
/// text itself carried, so the card's own are never counted as hidden.
pub const ISOLATE_START: char = '\u{2068}';
pub const ISOLATE_END: char = '\u{2069}';

/// What a card adds to text it quotes, in its reader's language. Each marker
/// stands outside the quotation marks, where quoted text cannot forge one:
/// the text's own quote marks are neutralized.
pub trait QuoteMarkers {
    /// Wraps `text` in the card's quotation marks.
    fn quote(&self, text: &str) -> String;
    /// Stands between the quoted start and end: `n` characters left out.
    fn elided(&self, n: usize) -> String;
    /// Follows the quote: `n` invisible characters the agent receives but
    /// the card cannot show.
    fn hidden(&self, n: usize) -> String;
}

/// Flatten an agent-authored title to one bounded line, as [`one_line`]
/// does, so a multi-line title cannot break the row it is rendered on, and no
/// consumer has to defend against one.
pub fn sanitize_title(raw: &str) -> Option<String> {
    let clean: String = one_line(raw).chars().take(TITLE_MAX_CHARS).collect();
    (!clean.is_empty()).then_some(clean)
}

/// A prompt as an approval card quotes it, so nothing the agent receives is
/// out of sight: flattened as [`one_line`] does, framing quotes neutralized,
/// quoted whole or, past the bound, its two quoted ends around
/// `markers.elided`, then `markers.hidden` for the invisible characters
/// dropped. Empty only when `raw` carries nothing at all.
pub fn prompt_excerpt(raw: &str, markers: &impl QuoteMarkers) -> String {
    card_quote(
        raw,
        PROMPT_EXCERPT_HEAD_CHARS,
        PROMPT_EXCERPT_TAIL_CHARS,
        markers,
    )
}

/// [`prompt_excerpt`] at each bound a card may need, widest first, down to
/// the floor. A card that must fit one message takes the first that does:
/// its other fields are bounded too, so the floor always fits.
pub fn prompt_excerpts<'a, M: QuoteMarkers>(
    raw: &'a str,
    markers: &'a M,
) -> impl Iterator<Item = String> + 'a {
    PROMPT_EXCERPT_BOUNDS
        .into_iter()
        .map(move |(head, tail)| card_quote(raw, head, tail, markers))
}

/// A title someone else wrote, as a card or a phone row quotes it: the same
/// as [`prompt_excerpt`], within the title's bound.
pub fn title_excerpt(raw: &str, markers: &impl QuoteMarkers) -> String {
    card_quote(
        raw,
        TITLE_EXCERPT_HEAD_CHARS,
        TITLE_EXCERPT_TAIL_CHARS,
        markers,
    )
}

/// [`prompt_excerpt`] within `head + tail` characters.
fn card_quote(raw: &str, head: usize, tail: usize, markers: &impl QuoteMarkers) -> String {
    let (line, hidden) = flatten(raw);
    let line: String = line
        .chars()
        .map(|c| {
            if FRAMING_QUOTES.contains(&c) {
                NEUTRAL_QUOTE
            } else {
                c
            }
        })
        .collect();
    let mut quoted = match split_ends(&line, head, tail) {
        Some((head, elided, tail)) => format!(
            "{}{}{}",
            markers.quote(&isolate(head)),
            markers.elided(elided),
            markers.quote(&isolate(tail))
        ),
        None if line.is_empty() => String::new(),
        None => markers.quote(&isolate(&line)),
    };
    if hidden > 0 {
        quoted.push_str(&markers.hidden(hidden));
    }
    quoted.trim().to_owned()
}

/// `text` between [`ISOLATE_START`] and [`ISOLATE_END`], as a card quotes it.
pub fn isolate(text: &str) -> String {
    format!("{ISOLATE_START}{text}{ISOLATE_END}")
}

/// Invisible characters are dropped, control characters become spaces, and
/// whitespace runs collapse to one space.
pub fn one_line(raw: &str) -> String {
    flatten(raw).0
}

/// [`one_line`], and how many invisible characters it dropped.
fn flatten(raw: &str) -> (String, usize) {
    let mut hidden = 0;
    let spaced: String = raw
        .chars()
        .filter(|c| {
            let drop = daruda_core::text::hides_or_reorders(*c);
            hidden += usize::from(drop);
            !drop
        })
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let line = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    (line, hidden)
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
