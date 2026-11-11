//! The file selection action rendered outside pane clips.

use crate::{ui::theme, workspace::Workspace};
use gpui::{
    AnyElement, Context, IntoElement, MouseButton, anchored, deferred, div, prelude::*, px,
};

pub(in crate::workspace) fn selection_action(
    ws: &Workspace,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    if ws.workspace_page.is_some() {
        return None;
    }
    let pane = ws.active_runtime().focused_pane_id;
    let file = ws
        .active_runtime()
        .panes
        .iter()
        .find(|p| p.id == pane)?
        .file_content()?;
    let action = file.selection_action.clone()?;
    let disabled = ws.file_input_refusal(cx).is_some();
    let send = ws.file_send_action();
    let reference = action.reference;
    let t = theme::current(cx);
    use crate::ui::Disableable as _;
    Some(
        deferred(
            anchored()
                .position(action.position)
                .snap_to_window_with_margin(px(theme::GAP_SM))
                .child(
                    div()
                        .bg(t.float_panel_bg)
                        .border_1()
                        .border_color(t.border)
                        .rounded(px(theme::RADIUS_SM))
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            crate::ui::button_compact("file-selection-add", send.label)
                                .icon(send.icon)
                                .tooltip(send.tooltip)
                                .disabled(disabled)
                                .debug_selector(|| "file-selection-add".into())
                                .on_click(cx.listener(move |ws, _, window, cx| {
                                    ws.insert_file_reference(
                                        pane,
                                        reference.as_ref().clone(),
                                        window,
                                        cx,
                                    )
                                })),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Layout is covered by isolated whole-workspace screenshot scenarios.
}
