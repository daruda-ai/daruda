use crate::surface::strings as s;

/// The finished-run line under a completion ping's header, from its
/// already-formatted segments (duration, "3 files edited", …).
pub(crate) fn remote_run_summary(segments: &[String]) -> String {
    s::control::run_summary(segments.join(" · "))
}

/// Replaces the generated `control::approval_prompt_elided`: English names
/// one character in the singular.
pub(crate) fn approval_prompt_elided(n: usize) -> String {
    match n {
        1 => s::control::approval_prompt_elided_one(),
        n => rust_i18n::t!("control.approval_prompt_elided", n = n).into_owned(),
    }
}

/// Replaces the generated `control::approval_hidden_chars`, as above.
pub(crate) fn approval_hidden_chars(n: usize) -> String {
    match n {
        1 => s::control::approval_hidden_chars_one(),
        n => rust_i18n::t!("control.approval_hidden_chars", n = n).into_owned(),
    }
}
