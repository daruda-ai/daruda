//! A settled permission request: a folded card that says how it ended,
//! holding the request as it was asked.

use daruda_acp::{PermissionItem, PermissionKindView, PermissionResolution};
use daruda_ui::theme::{self, PaneSurfaceTokens};
use gpui::{AnyElement, Context, IntoElement, SharedString, Window, div, prelude::*, px};

use super::content::{permission_icon, request_summary};
use super::request_title;
use crate::fold::FoldKey;
use crate::render::card_frame::{CardFrame, CardTone};
use crate::render::fold_header::{FoldHeader, FoldRow};
use crate::strings as s;
use crate::view::AgentChatView;

/// The settled form of [`super::permission_card`]: the same frame it waited
/// in, folded to one header line the way a finished tool card is, so a
/// request keeps its shape from asking to settled.
pub(super) fn permission_record(
    ix: usize,
    card: &PermissionItem,
    expanded: bool,
    dim: f32,
    window: &mut Window,
    cx: &mut Context<AgentChatView>,
) -> AnyElement {
    let surface = PaneSurfaceTokens::agent_chat(cx).dimmed(dim);
    let font = px(theme::agent_chat_font_size(cx));
    let (status, choice) = resolution_label(card);
    let (icon, color) = resolution_icon(card, &surface, dim);
    let header = FoldHeader::with_title(
        div()
            .flex()
            .items_center()
            .gap(px(theme::GAP_STANDARD))
            .text_size(font)
            .child(permission_icon(
                "agent-chat-permission-result-icon",
                ix,
                icon,
                color,
            ))
            .child(
                div()
                    .flex_none()
                    .text_color(surface.foreground)
                    .child(SharedString::from(status)),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(surface.foreground_muted)
                    .child(SharedString::from(request_title(card))),
            )
            .into_any_element(),
    );
    // The choice sits where a tool card shows its status, so a long title
    // truncates without hiding which grant was given.
    let header = match choice {
        Some(choice) => header.trailing(
            div()
                .debug_selector(move || format!("agent-chat-permission-choice-{ix}"))
                .flex_none()
                .text_size(font)
                .text_color(surface.foreground_muted)
                .child(SharedString::from(choice))
                .into_any_element(),
        ),
        None => header,
    };
    CardFrame::new(CardTone::Standard)
        .render(dim, cx)
        .debug_selector(move || format!("agent-chat-permission-{ix}"))
        .text_size(font)
        .text_color(surface.foreground)
        .child(
            FoldRow::block(
                SharedString::from(format!("agent-chat-permission-record-{ix}")),
                FoldKey::Permission(ix),
                expanded,
                header,
                move |cx| record_details(ix, card, &surface, font, window, cx),
            )
            .render(dim, cx),
        )
        .into_any_element()
}

/// The request as it was asked: the full title, which the header truncates,
/// and the input summary only when it says more than that title.
fn record_details(
    ix: usize,
    card: &PermissionItem,
    surface: &PaneSurfaceTokens,
    font: gpui::Pixels,
    window: &mut Window,
    cx: &mut Context<AgentChatView>,
) -> AnyElement {
    let title = request_title(card);
    let summary = card
        .raw_input_summary
        .as_ref()
        .filter(|summary| !summary.trim().is_empty())
        .filter(|summary| !summary_restates_title(&title, summary));
    div()
        .flex()
        .flex_col()
        .gap(px(theme::AGENT_CHAT_MSG_GAP))
        .child(
            div()
                .debug_selector(move || format!("agent-chat-permission-record-title-{ix}"))
                .min_w_0()
                .child(
                    daruda_ui::selectable_text(
                        ("agent-chat-perm-record-title", ix),
                        SharedString::from(title),
                    )
                    .color(surface.foreground),
                ),
        )
        .children(
            summary
                .map(|summary| request_summary(ix, summary, surface, window, cx).text_size(font)),
        )
        .into_any_element()
}

/// Whether the adapter's input summary (`label: value`, its value possibly cut
/// short with `…`) only repeats the request title, as a shell command
/// request's title usually is the command itself.
fn summary_restates_title(title: &str, summary: &str) -> bool {
    let value = summary.split_once(": ").map_or(summary, |(_, value)| value);
    let value = value.strip_suffix('…').unwrap_or(value).trim();
    !value.is_empty() && title.contains(value)
}

pub(super) fn resolution_label(card: &PermissionItem) -> (String, Option<String>) {
    match &card.resolved {
        Some(PermissionResolution::Chosen(id)) => {
            let choice = card.options.iter().find(|choice| &choice.option_id == id);
            let status = match choice.map(|choice| choice.kind) {
                Some(PermissionKindView::AllowOnce | PermissionKindView::AllowAlways) => {
                    s::agent_chat::permission_allowed()
                }
                Some(PermissionKindView::RejectOnce | PermissionKindView::RejectAlways) => {
                    s::agent_chat::permission_denied()
                }
                None => s::agent_chat::permission_resolved(),
            };
            (
                status,
                Some(choice.map_or_else(|| id.clone(), |choice| choice.name.clone())),
            )
        }
        Some(PermissionResolution::Cancelled) => (s::agent_chat::permission_cancelled(), None),
        None => (s::agent_chat::permission_waiting(), None),
    }
}

pub(super) fn resolution_icon(
    card: &PermissionItem,
    surface: &PaneSurfaceTokens,
    dim: f32,
) -> (&'static str, gpui::Hsla) {
    use daruda_ui::icons;
    let choice = match &card.resolved {
        Some(PermissionResolution::Chosen(id)) => card.options.iter().find(|o| &o.option_id == id),
        _ => None,
    };
    match choice.map(|choice| choice.kind) {
        Some(PermissionKindView::AllowOnce | PermissionKindView::AllowAlways) => (
            icons::CHECK,
            theme::dim_toward_gray(
                if surface.syntax_is_light {
                    theme::AGENT_READING_LIGHT
                } else {
                    theme::AGENT_READING
                },
                dim,
            ),
        ),
        Some(PermissionKindView::RejectOnce | PermissionKindView::RejectAlways) => {
            (icons::CLOSE, surface.foreground_muted)
        }
        None if card.resolved == Some(PermissionResolution::Cancelled) => {
            (icons::MINIMIZE, surface.foreground_muted)
        }
        None => (icons::INFO, surface.foreground_muted),
    }
}

#[cfg(test)]
mod tests {
    use super::summary_restates_title;

    #[test]
    fn a_summary_that_only_repeats_the_title_is_dropped() {
        assert!(summary_restates_title("git status", "command: git status"));
        // The adapter cuts long values short; the kept prefix still repeats.
        assert!(summary_restates_title(
            "cd /repo && cargo test --workspace",
            "command: cd /repo && cargo…"
        ));
        assert!(summary_restates_title("Write /tmp/a.rs", "file: /tmp/a.rs"));
    }

    #[test]
    fn a_summary_that_adds_detail_is_kept() {
        assert!(!summary_restates_title("Write file", "file: /tmp/a.rs"));
        assert!(!summary_restates_title(
            "Permission required",
            "command: ls"
        ));
        assert!(!summary_restates_title("anything", "command: …"));
    }
}
