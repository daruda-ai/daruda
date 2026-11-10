use super::*;
use daruda_control_types::agent_text::{
    PROMPT_EXCERPT_FLOOR_HEAD_CHARS, PROMPT_EXCERPT_FLOOR_TAIL_CHARS, PROMPT_EXCERPT_HEAD_CHARS,
    PROMPT_EXCERPT_TAIL_CHARS, isolate,
};

/// A prompt as a card of nothing else quotes it.
fn prompt_phrase(prompt: &str) -> Option<String> {
    with_prompt(prompt, str::to_owned)
}

#[test]
fn the_agent_phrase_names_the_agent_and_its_requested_mode() {
    let disclosure = ChatDisclosure {
        agent_name: "Claude Code".into(),
        mode: "Bypass Permissions".into(),
        ..ChatDisclosure::default()
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
    assert_eq!(
        prompt_phrase("fix\nit"),
        Some(s::control::approval_quoted(isolate("fix it")))
    );
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
    let counted = format!(
        "{}{}",
        s::control::approval_quoted(isolate("fix")),
        s::control::approval_hidden_chars(9)
    );
    assert_eq!(prompt_phrase(&tagged), Some(counted.clone()));
    assert_eq!(quoted_title(&tagged), counted);
    let marker = s::control::approval_hidden_chars(1);
    let marker = marker.trim();
    assert!(!quoted_title("Fix the login").contains(marker));
    assert_eq!(
        quoted_title("Fix the login"),
        s::control::approval_quoted(isolate("Fix the login"))
    );
}

/// One left-out or hidden character is named in the singular.
#[test]
fn a_count_of_one_takes_the_singular_marker() {
    assert_eq!(
        s::control::approval_hidden_chars(1),
        s::control::approval_hidden_chars_one()
    );
    assert_eq!(
        s::control::approval_prompt_elided(1),
        s::control::approval_prompt_elided_one()
    );
    assert_ne!(
        s::control::approval_hidden_chars(2),
        s::control::approval_hidden_chars_one()
    );
    assert!(s::control::approval_prompt_elided(2).contains('2'));
}

/// A long title reaches the agent whole, so the card quotes both ends.
#[test]
fn a_long_title_quotes_its_end_and_the_hidden_count() {
    use daruda_control_types::agent_text::{TITLE_EXCERPT_HEAD_CHARS, TITLE_EXCERPT_TAIL_CHARS};
    let head = "a".repeat(TITLE_EXCERPT_HEAD_CHARS);
    let tail = "z".repeat(TITLE_EXCERPT_TAIL_CHARS);
    assert_eq!(
        quoted_title(&format!("{head}{}{tail}", "m".repeat(50))),
        format!(
            "{}{}{}",
            s::control::approval_quoted(isolate(&head)),
            s::control::approval_prompt_elided(50),
            s::control::approval_quoted(isolate(&tail))
        )
    );
}

/// The end of a long prompt is quoted too, with the localized count of
/// what lies between: the first 1000 and last 500 characters while the
/// card fits one phone message.
#[test]
fn a_long_prompt_quotes_its_end_and_the_hidden_count() {
    let head = "a".repeat(PROMPT_EXCERPT_HEAD_CHARS);
    let tail = "z".repeat(PROMPT_EXCERPT_TAIL_CHARS);
    let prompt = format!("{head}{}{tail}", "m".repeat(9));
    let whole = format!(
        "{}{}{}",
        s::control::approval_quoted(isolate(&head)),
        s::control::approval_prompt_elided(9),
        s::control::approval_quoted(isolate(&tail))
    );
    assert_eq!(prompt_phrase(&prompt), Some(whole.clone()));
    assert_eq!(
        with_prompt(&prompt, |p| s::control::approval_lane_create_prompt(
            "“w”",
            "Claude Code",
            p
        )),
        Some(s::control::approval_lane_create_prompt(
            "“w”",
            "Claude Code",
            whole
        ))
    );
}

/// A prompt whose widest excerpt would push the card past one phone
/// message is quoted narrower, down to the floor, never over the limit
/// while the floor fits.
#[test]
fn a_card_too_long_for_one_message_quotes_less_of_its_prompt() {
    // Astral characters are two UTF-16 units each: 1500 of them beside
    // a long sentence exceed the limit, and the next bound is taken.
    let prompt = "😀".repeat(4000);
    let sentence = "x".repeat(1000);
    let card = with_prompt(&prompt, |p| format!("{sentence}{p}")).expect("a prompt");
    assert!(fits_one_message(&card), "{}", card.encode_utf16().count());
    assert_eq!(card.chars().filter(|c| *c == '😀').count(), 750);
    // A sentence long enough to leave room only for the floor gets it.
    let filler = "x".repeat(CARD_MAX_UTF16 - 600);
    let card = with_prompt(&prompt, |p| format!("{filler}{p}")).expect("a prompt");
    let floor = format!(
        "{filler}{}{}{}",
        s::control::approval_quoted(isolate(&"😀".repeat(PROMPT_EXCERPT_FLOOR_HEAD_CHARS))),
        s::control::approval_prompt_elided(
            4000 - PROMPT_EXCERPT_FLOOR_HEAD_CHARS - PROMPT_EXCERPT_FLOOR_TAIL_CHARS
        ),
        s::control::approval_quoted(isolate(&"😀".repeat(PROMPT_EXCERPT_FLOOR_TAIL_CHARS))),
    );
    assert_eq!(card, floor);
}
