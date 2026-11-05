//! Selectable request details, input-type glyphs and the summary copy action.

use super::*;
use daruda_ui::{Sizable as _, icons};
use gpui::Div;

pub(super) fn permission_icon(
    prefix: &'static str,
    ix: usize,
    path: &'static str,
    color: gpui::Hsla,
) -> Div {
    div()
        .debug_selector(move || format!("{prefix}-{ix}"))
        .flex_none()
        .size(px(METRICS.icon_size))
        .text_color(color)
        .child(
            icons::icon(path)
                .with_size(px(METRICS.icon_size))
                .text_color(color),
        )
}

pub(super) fn request_details(
    ix: usize,
    card: &PermissionItem,
    surface: &PaneSurfaceTokens,
    window: &mut Window,
    cx: &mut Context<AgentChatView>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(METRICS.content_gap))
        .child(
            div()
                .debug_selector(move || format!("agent-chat-permission-title-{ix}"))
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(px(theme::FONT_SIZE_LG))
                .line_height(relative(METRICS.title_line_height))
                .child(
                    daruda_ui::selectable_text(
                        ("agent-chat-perm-title", ix),
                        SharedString::from(request_title(card)),
                    )
                    .color(surface.foreground),
                ),
        )
        .children(
            card.raw_input_summary
                .as_ref()
                .filter(|text| !text.trim().is_empty())
                .map(|summary| {
                    let copy = daruda_ui::button_with_surface(
                        daruda_ui::copy_button(
                            SharedString::from(format!("agent-chat-permission-copy-{ix}")),
                            summary.clone().into(),
                            icons::icon(icons::COPY),
                            s::agent_chat::permission_copy_summary().into(),
                            icons::icon(icons::CHECK),
                            s::agent_chat::code_copied().into(),
                            window,
                            cx,
                        ),
                        surface,
                        cx,
                    )
                    .debug_selector(move || format!("agent-chat-permission-copy-{ix}"))
                    .size(px(METRICS.copy_size))
                    .p_0()
                    .flex_none();
                    div()
                        .debug_selector(move || format!("agent-chat-permission-input-{ix}"))
                        .w_full()
                        .min_w_0()
                        .min_h(px(METRICS.summary_min_height))
                        .flex()
                        .items_center()
                        .gap(px(theme::GAP_LG))
                        .px(px(METRICS.summary_pad_x))
                        .py(px(theme::PAD_STANDARD))
                        .text_size(px(theme::FONT_SIZE_SM))
                        .line_height(relative(METRICS.line_height))
                        .rounded(px(METRICS.summary_radius))
                        .bg(surface.background)
                        .border_1()
                        .border_color(surface.border_tint)
                        .child(permission_icon(
                            "agent-chat-permission-input-icon",
                            ix,
                            input_icon(summary),
                            surface.foreground_muted,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(theme::FONT_FAMILY_MONOSPACE)
                                .child(
                                    daruda_ui::selectable_text(
                                        ("agent-chat-perm-input", ix),
                                        SharedString::from(summary.clone()),
                                    )
                                    .color(surface.foreground),
                                ),
                        )
                        .child(copy)
                }),
        )
}

fn input_icon(summary: &str) -> &'static str {
    match summary.split_once(": ").map(|(kind, _)| kind) {
        Some("command") => icons::TERMINAL,
        Some("file" | "path") => icons::FILE_VIEWER,
        _ => icons::INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_glyph_uses_summary_kind_without_guessing_from_its_value() {
        assert_eq!(input_icon("command: cat file.rs"), icons::TERMINAL);
        assert_eq!(input_icon("file: /tmp/a.rs"), icons::FILE_VIEWER);
        assert_eq!(input_icon("path: /tmp"), icons::FILE_VIEWER);
        assert_eq!(input_icon("opaque"), icons::INFO);
    }
}
