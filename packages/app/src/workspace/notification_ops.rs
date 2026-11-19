//! Navigation from an OS notification to a still-live pane.

use gpui::{Context, Window};

use super::Workspace;

impl Workspace {
    pub(crate) fn reveal_notification_pane(
        &mut self,
        pane: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, lane)) = self
            .pane_lane_index()
            .into_iter()
            .find(|(id, _)| id.0 == pane)
        else {
            return;
        };
        if self.active != lane {
            self.activate_lane(lane, window, cx);
        }
        if let Some(index) = self
            .active_runtime()
            .tabs
            .iter()
            .position(|tab| tab.layout.contains(pane.into()))
        {
            self.activate_tab(index, window, cx);
        }
        self.set_focused_pane(pane.into(), window, cx);
        self.focus_pane(pane.into(), window, cx);
        cx.notify();
    }
}
