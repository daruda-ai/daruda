//! The chat's own wording on `daruda_ui`'s label-taking widgets.

use daruda_ui::{Button, CopyTooltips, Markdown, button_icon, copy_button, icons};
use gpui::{App, ElementId, IntoElement, SharedString, Window};

use crate::strings as s;

/// Close action, tooltipped in the chat's own words.
pub(crate) fn button_close(id: impl Into<ElementId>, cx: &App) -> Button {
    button_icon(id, icons::CLOSE, cx).tooltip(s::agent_chat::close())
}

/// A copy-to-clipboard button for a rendered code block. `id` must be stable
/// per block across renders, or the keyed ✓ feedback state resets.
pub(crate) fn code_copy_button<I: Into<ElementId>>(
    id: I,
    code: SharedString,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<I> {
    let tooltips = copy_tooltips();
    copy_button(
        id,
        code,
        icons::icon(icons::COPY),
        tooltips.copy,
        icons::icon(icons::CHECK),
        tooltips.copied,
        window,
        cx,
    )
}

/// A copy-to-clipboard button for a whole message, coloured from the pane it
/// sits on: `ghost` would take the UI theme's foreground, which has no
/// contrast guarantee on the terminal-mirrored pane. `id` must be stable per
/// message across renders.
pub(crate) fn message_copy_button<I: Into<ElementId>>(
    id: I,
    text: SharedString,
    surface: &daruda_ui::theme::PaneSurfaceTokens,
    window: &mut Window,
    cx: &mut App,
) -> Button {
    let button = copy_button(
        id,
        text,
        icons::icon(icons::COPY),
        s::agent_chat::message_copy().into(),
        icons::icon(icons::CHECK),
        s::agent_chat::code_copied().into(),
        window,
        cx,
    );
    daruda_ui::button_with_surface(button, surface, cx)
}

/// Rendered, drag-selectable Markdown whose code blocks carry a copy button.
pub(crate) fn markdown(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Markdown {
    daruda_ui::markdown(id, text, copy_tooltips())
}

fn copy_tooltips() -> CopyTooltips {
    CopyTooltips {
        copy: s::agent_chat::code_copy().into(),
        copied: s::agent_chat::code_copied().into(),
    }
}
