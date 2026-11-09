//! What an approval card discloses about the chat it would open: the agent,
//! the permission mode it asks for and its first prompt. A delegated chat
//! approves its own tools, so a card naming only a title could hide what the
//! agent is about to be told. The prompt is quoted through
//! `agent_text::prompt_excerpt`, the bound shared with agent-authored titles.

use daruda_control_types::agent_text::prompt_excerpt;

use crate::surface::strings as s;
use crate::workspace::ChatDisclosure;

/// "Claude Code (requested permission mode: Bypass Permissions)".
pub(super) fn agent_phrase(disclosure: &ChatDisclosure) -> String {
    s::control::approval_agent(&disclosure.agent_name, &disclosure.mode)
}

/// A prompt as a card quotes it: one line, its start and end around how much
/// was left out. `None` when nothing visible is left, so the card says it
/// sends no prompt rather than quoting an empty one.
pub(super) fn prompt_phrase(prompt: &str) -> Option<String> {
    let excerpt = prompt_excerpt(prompt, s::control::approval_prompt_elided);
    (!excerpt.is_empty()).then_some(excerpt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daruda_control_types::agent_text::{PROMPT_EXCERPT_HEAD_CHARS, PROMPT_EXCERPT_TAIL_CHARS};

    #[test]
    fn the_agent_phrase_names_the_agent_and_its_requested_mode() {
        let disclosure = ChatDisclosure {
            agent_name: "Claude Code".into(),
            mode: "Bypass Permissions".into(),
        };
        assert_eq!(
            agent_phrase(&disclosure),
            s::control::approval_agent("Claude Code", "Bypass Permissions")
        );
    }

    /// Whitespace and invisible characters alone quote as no prompt at all.
    #[test]
    fn an_empty_or_invisible_prompt_is_no_prompt() {
        for blank in ["", "  \n\t", "\u{200B}\u{202E}"] {
            assert_eq!(prompt_phrase(blank), None, "{blank:?}");
        }
        assert_eq!(prompt_phrase("fix\nit").as_deref(), Some("fix it"));
    }

    /// The end of a long prompt is quoted too, with the localized count of
    /// what lies between.
    #[test]
    fn a_long_prompt_quotes_its_end_and_the_hidden_count() {
        let head = "a".repeat(PROMPT_EXCERPT_HEAD_CHARS);
        let tail = "z".repeat(PROMPT_EXCERPT_TAIL_CHARS);
        let prompt = format!("{head}{}{tail}", "m".repeat(9));
        assert_eq!(
            prompt_phrase(&prompt),
            Some(format!(
                "{head}{}{tail}",
                s::control::approval_prompt_elided(9)
            ))
        );
    }
}
