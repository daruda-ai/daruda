//! What an approval card discloses about the chat it would open: the agent,
//! the permission mode it asks for and the start of its first prompt. A
//! delegated chat approves its own tools, so a card naming only a title could
//! hide what the agent is about to be told.

use crate::surface::strings as s;
use crate::workspace::ChatDisclosure;

/// The most of a prompt a card quotes. Enough to recognize the request on a
/// phone screen; the card is one line, and the full text is the agent's.
const PROMPT_EXCERPT_CHARS: usize = 120;

/// "Claude Code (permission mode: Bypass Permissions)".
pub(super) fn agent_phrase(disclosure: &ChatDisclosure) -> String {
    s::control::approval_agent(&disclosure.agent_name, &disclosure.mode)
}

/// `prompt` on one line, cut to [`PROMPT_EXCERPT_CHARS`] with an ellipsis.
/// Control characters and whitespace runs become single spaces, so a prompt
/// cannot break the card's line or push the question off it.
pub(super) fn prompt_excerpt(prompt: &str) -> String {
    let line = prompt
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    match daruda_core::text::char_prefix(&line, PROMPT_EXCERPT_CHARS) {
        (prefix, true) => format!("{prefix}…"),
        (whole, false) => whole.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_prompt_is_quoted_whole_on_one_line() {
        assert_eq!(
            prompt_excerpt("fix\n\tthe  login\r\nbug"),
            "fix the login bug"
        );
    }

    #[test]
    fn a_long_prompt_is_cut_at_the_cap_with_an_ellipsis() {
        let prompt = "가".repeat(PROMPT_EXCERPT_CHARS + 5);
        let excerpt = prompt_excerpt(&prompt);
        assert_eq!(excerpt.chars().count(), PROMPT_EXCERPT_CHARS + 1);
        assert!(excerpt.ends_with('…'));
        assert_eq!(
            prompt_excerpt(&"a".repeat(PROMPT_EXCERPT_CHARS))
                .chars()
                .count(),
            PROMPT_EXCERPT_CHARS
        );
    }
}
