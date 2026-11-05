//! Fenced-code palettes follow their host surface, independently of UI chrome.

use crate::theme::{self, SyntaxPalette};
use gpui::App;
use gpui_component::highlighter::HighlightTheme;
use gpui_component::text::SharedCodeBlockHighlighter;
use std::sync::Arc;

struct SurfaceThemes {
    palette: SyntaxPalette,
    dark: Arc<HighlightTheme>,
    light: Arc<HighlightTheme>,
}

impl gpui::Global for SurfaceThemes {}

impl SurfaceThemes {
    fn new(palette: SyntaxPalette) -> Self {
        let build = |is_light| {
            let mut highlight = (*HighlightTheme::default_dark()).clone();
            highlight.style.editor_foreground =
                Some(theme::syntax_theme_of(palette, is_light).default);
            highlight.style.syntax = theme::editor_syntax_colors_of(palette, is_light);
            Arc::new(highlight)
        };
        Self {
            palette,
            dark: build(false),
            light: build(true),
        }
    }
}

pub(super) fn for_surface(is_light: bool, cx: &mut App) -> Arc<SharedCodeBlockHighlighter> {
    let palette = theme::active_syntax_palette(cx);
    if cx
        .try_global::<SurfaceThemes>()
        .is_none_or(|themes| themes.palette != palette)
    {
        cx.set_global(SurfaceThemes::new(palette));
    }
    let themes = cx.global::<SurfaceThemes>();
    gpui_component::text::shared_code_block_highlighter(if is_light {
        &themes.light
    } else {
        &themes.dark
    })
}

#[cfg(test)]
mod tests {
    use crate::theme;

    #[gpui::test]
    fn fenced_code_uses_the_host_palette_and_reuses_its_callback(cx: &mut gpui::TestAppContext) {
        crate::test_support::init_gpui_component(cx);
        cx.update(|cx| {
            for ui in ["daruda_light", "daruda_dark"] {
                theme::apply_ui_theme(ui, cx);
                for is_light in [false, true] {
                    let highlighter = super::for_surface(is_light, cx);
                    assert!(std::sync::Arc::ptr_eq(
                        &highlighter,
                        &super::for_surface(is_light, cx)
                    ));
                    let block =
                        gpui_base::text::CodeBlock::from_code("let count = 42;", Some("rust"));
                    let highlights = highlighter(&block);
                    let expected =
                        theme::syntax_theme_of(theme::active_syntax_palette(cx), is_light);
                    assert!(
                        highlights.iter().any(|(range, style)| {
                            &block.code()[range.clone()] == "42"
                                && style.color == Some(expected.constant)
                        }),
                        "numeric tokens must use the host palette under {ui}"
                    );
                }
            }
            let previous = super::for_surface(false, cx);
            theme::set_active_syntax_palette(cx, theme::SyntaxPalette::OneDark);
            let changed = super::for_surface(false, cx);
            assert!(!std::sync::Arc::ptr_eq(&previous, &changed));
            let block = gpui_base::text::CodeBlock::from_code("let count = 42;", Some("rust"));
            let expected = theme::syntax_theme_of(theme::SyntaxPalette::OneDark, false);
            assert!(changed(&block).iter().any(|(range, style)| {
                &block.code()[range.clone()] == "42" && style.color == Some(expected.constant)
            }));
        });
    }
}
