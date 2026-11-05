//! Theme reload must preserve an in-progress answer and composer focus.
use super::composer::{
    add_pane, ask, color_then_reason, composer_text, connect, type_text, workspace,
};
use gpui::{Focusable as _, TestAppContext};

#[gpui::test]
fn theme_reload_preserves_question_draft_and_focus(cx: &mut TestAppContext) {
    let (_dir, ws, mut vcx) = workspace(cx);
    let pane = add_pane(&ws, &mut vcx);
    let _probes = connect(&ws, &mut vcx, pane, &[1]);
    ask(&ws, &mut vcx, pane, 1, color_then_reason());
    type_text(&ws, &mut vcx, "Keep this answer");
    let before = vcx.update(|window, cx| {
        let snapshot = {
            let ws = ws.read(cx);
            let view = ws.agent_chat_view(pane).unwrap().read(cx);
            let focus = ws.input_dock.input.read(cx).focus_handle(cx);
            (
                view.question_composer(),
                view.question_focus_for_test(),
                focus,
            )
        };
        window.focus(&snapshot.2, cx);
        snapshot
    });
    for ui in ["daruda_light", "daruda_dark"] {
        vcx.update(|_, cx| {
            assert!(crate::ui::theme::apply_ui_theme(ui, cx));
            assert_eq!(crate::ui::theme::current(cx).is_dark(), ui == "daruda_dark");
            let mut config = daruda_config::Config::default();
            config.theme.ui_preset = ui.into();
            config.theme.terminal_preset = "custom".into();
            config.colors.background = daruda_config::HexColor::new(249, 250, 251);
            config.colors.foreground = daruda_config::HexColor::new(26, 29, 36);
            ws.update(cx, |ws, cx| ws.reload_config(&config, cx));
            assert_eq!(
                crate::ui::theme::agent_chat_bg(cx),
                gpui::Hsla::from(gpui::rgb(0xf9fafb))
            );
        });
        vcx.run_until_parked();
        assert_eq!(composer_text(&ws, &mut vcx), "Keep this answer");
        vcx.update(|window, cx| {
            let view = ws.read(cx).agent_chat_view(pane).unwrap().read(cx);
            assert_eq!(view.question_composer(), before.0);
            assert_eq!(view.question_focus_for_test(), before.1);
            assert!(before.2.is_focused(window));
        });
    }
}
