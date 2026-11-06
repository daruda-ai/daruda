//! A question the agent asked from a tool call, drawn on that call's row: the
//! open form while it waits (`card`), and once settled a folded summary bar
//! (`record`). Options are list rows (number, glyph, label), not chips: the
//! card is conversation structure, and a picked or highlighted state reads in
//! its glyph as well as its fill.

use daruda_acp::{QuestionField, QuestionFieldKind};
use daruda_ui::theme::{self, PaneSurfaceTokens};
use gpui::{Div, SharedString, div, prelude::*, px};

use crate::strings as s;

mod card;
mod message;
mod record;

pub(in crate::render) use card::{question_card, question_form};
pub(in crate::render) use record::question_record;

pub(super) const GLYPH_HIGHLIGHT: &str = "▸";
const GLYPH_RADIO_OFF: &str = "○";
const GLYPH_RADIO_ON: &str = "◉";
const GLYPH_CHECK_OFF: &str = "☐";
const GLYPH_CHECK_ON: &str = "☑";
const GLYPH_WRITE: &str = "✎";

/// A field's title (with the multi-select marker) and description, as the
/// top of its column. `scope` keys their selection state to this form.
pub(super) fn field_heading(
    scope: &str,
    field: &QuestionField,
    surface: &PaneSurfaceTokens,
) -> Div {
    let multi = matches!(field.kind, QuestionFieldKind::MultiSelect(_));
    let mut title = field.title.clone().unwrap_or_default();
    if multi {
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(&s::agent_chat::question_multi());
    }
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_XS))
        .when(!title.is_empty(), |block| {
            block.child(
                div().whitespace_normal().child(
                    daruda_ui::selectable_text(text_id(scope, &field.key, "title"), title)
                        .color(surface.foreground_muted),
                ),
            )
        })
        .children(field.description.as_ref().map(|description| {
            div().whitespace_normal().child(
                daruda_ui::selectable_text(
                    text_id(scope, &field.key, "description"),
                    description.clone(),
                )
                .color(surface.foreground_subtle),
            )
        }))
}

/// Where an open form's selectable text is keyed: the card and the fallback
/// panel draw the same open question, never both at once.
pub(super) fn form_scope(question: u64) -> String {
    format!("agent-chat-question-{question}")
}

/// A stable selection key for one `part` of a form: keyed state, and the
/// selection with it, resets whenever an id changes between renders.
pub(super) fn text_id(scope: &str, key: &str, part: impl std::fmt::Display) -> SharedString {
    SharedString::from(format!("{scope}-{key}-{part}"))
}

/// The leading cells every row shares: the highlight cursor (kept in the
/// layout while hidden, so rows do not shift), the number, the glyph.
pub(super) fn row_frame(
    number: Option<usize>,
    highlighted: bool,
    glyph: &'static str,
    surface: &PaneSurfaceTokens,
) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(theme::GAP_SM))
        .px(px(theme::PAD_XS))
        .rounded(px(theme::RADIUS_SM))
        .child(
            div()
                .flex_none()
                .when(!highlighted, |el| el.invisible())
                .child(GLYPH_HIGHLIGHT),
        )
        .children(number.map(|number| {
            div()
                .flex_none()
                .text_color(surface.foreground_subtle)
                .child(SharedString::from(number.to_string()))
        }))
        .child(div().flex_none().child(daruda_ui::icons::icon(match glyph {
            GLYPH_RADIO_OFF => daruda_ui::icons::RADIO_OFF,
            GLYPH_RADIO_ON => daruda_ui::icons::RADIO_ON,
            GLYPH_CHECK_OFF => daruda_ui::icons::CHECKBOX_OFF,
            GLYPH_CHECK_ON => daruda_ui::icons::CHECKBOX_ON,
            _ => daruda_ui::icons::EDIT,
        })))
}

pub(super) fn choice_glyph(multi: bool, picked: bool) -> &'static str {
    match (multi, picked) {
        (false, false) => GLYPH_RADIO_OFF,
        (false, true) => GLYPH_RADIO_ON,
        (true, false) => GLYPH_CHECK_OFF,
        (true, true) => GLYPH_CHECK_ON,
    }
}

/// The bar marking the field being answered and a picked option. It is that
/// field's only cue, so it keeps the UI theme's link hue — resolved against the
/// pane like every other colour on it — rather than a neutral edge.
pub(in crate::render) fn question_accent(cx: &gpui::App) -> gpui::Hsla {
    theme::agent_chat_link_color(cx)
}

#[cfg(test)]
mod tests;
