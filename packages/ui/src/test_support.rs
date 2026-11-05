//! The globals a widget test needs before it renders anything.

use gpui::TestAppContext;

pub fn init_gpui_component(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
        // DarudaTheme must be registered before apply_daruda_palette, which
        // reads it to map slots into `gpui_component::Theme`.
        crate::theme::DarudaTheme::init(cx);
        crate::theme::apply_daruda_palette(cx);
    });
}

/// Last painted visible text bounds, for input tests without guessed coordinates.
pub fn painted_text_bounds(
    window: &gpui::Window,
    text: &str,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    gpui_base::test_support::texts(window)
        .into_iter()
        .find(|snapshot| snapshot.visible() && snapshot.text() == text)
        .map(|snapshot| snapshot.bounds())
}
