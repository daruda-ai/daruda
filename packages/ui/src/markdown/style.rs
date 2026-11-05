//! Host colors and typography for the shared Markdown renderer.
use super::Markdown;
use crate::theme;
use gpui::{
    App, HighlightStyle, Hsla, Pixels, Refineable as _, StyleRefinement, Styled as _, relative,
    rems,
};
use gpui_component::ActiveTheme as _;

impl Markdown {
    pub(super) fn text_view_style(
        &self,
        color: Hsla,
        surface: Hsla,
        rem_size: Pixels,
        cx: &App,
    ) -> gpui_base::text::TextViewStyle {
        let line = theme::neutral_tint_over(surface, theme::MD_VIEW_STRUCTURAL_LINE_ALPHA);
        let mut style = gpui_component::text::base_text_view_style(cx.theme())
            .with_foreground(color)
            // A blockquote is de-emphasised against the body colour, so it
            // tracks the surface and any dimming the caller applied.
            .with_muted_foreground(color.opacity(theme::MD_VIEW_MUTED_ALPHA))
            .with_code_background(theme::neutral_tint_over(
                surface,
                theme::MD_VIEW_CODE_FILL_ALPHA,
            ))
            .with_border(line)
            .with_inline_code(HighlightStyle {
                background_color: Some(theme::neutral_tint_over(
                    surface,
                    theme::MD_VIEW_INLINE_CODE_TINT_ALPHA,
                )),
                ..Default::default()
            })
            .with_table_head(
                StyleRefinement::default()
                    .bg(theme::neutral_tint_over(
                        surface,
                        theme::MD_VIEW_TABLE_HEADER_FILL_ALPHA,
                    ))
                    .text_color(color),
            );
        if self.surface.is_some() {
            let table = style.table().clone().bg(theme::neutral_tint_over(
                surface,
                theme::MD_VIEW_CODE_FILL_ALPHA,
            ));
            style = style
                .with_table(table)
                .with_link(theme::link_on(surface, cx.theme().link))
                .with_dark(surface.l < 0.5);
        }
        // With a configured body size, the vertical rhythm follows it: the
        // paragraph gap, the heading scale, and code rows on the prose pitch.
        let line_height = self.line_height.unwrap_or(theme::MD_VIEW_LINE_HEIGHT);
        if let Some(size) = self.text_size {
            style = style.with_paragraph_gap(rems(size * theme::MD_VIEW_PARAGRAPH_GAP / rem_size));
            style = style.with_heading(move |level| {
                let scale = theme::MD_VIEW_HEADING_SCALE[usize::from(level.clamp(1, 6)) - 1];
                StyleRefinement::default()
                    .text_size(size * scale)
                    .line_height(relative(line_height))
            });
            let mut code_block = style.code_block().clone();
            code_block.refine(
                &StyleRefinement::default()
                    .text_size(size)
                    .line_height(relative(line_height)),
            );
            style = style.with_code_block(code_block);
        }
        if let Some(size) = self.heading_size {
            style = style.with_heading(move |_| StyleRefinement::default().text_size(size));
        }
        style
    }
}

#[cfg(test)]
#[path = "theme_tests.rs"]
mod tests;
