//! Whether a pane is in view: the question the "skip the pane you are looking
//! at" notification rule asks of every channel.

use super::pane_tree::PaneId;
use crate::workspace::Workspace;

impl Workspace {
    /// `true` when `pane_id` is drawn: the side group's shown pane, or in the
    /// active lane's active tab, not
    /// hidden behind another pane zoomed in that tab, and not covered by
    /// Settings, a page or an unavailable lane's empty state. Every pane of a
    /// split counts, not only the focused one. Mirrors `render::center`.
    pub(in crate::workspace) fn pane_on_screen(&self, pane_id: PaneId) -> bool {
        if self.settings.is_some() || self.active_page().is_some() {
            return false;
        }
        if self.active_lane().is_some_and(|lane| {
            lane.availability != crate::lane::availability::LaneAvailability::Present
        }) {
            return false;
        }
        let runtime = self.active_runtime();
        let shown = self.groups_shown();
        if let Some(super::side_group::PanePlace::Side(index)) = runtime.place_of(pane_id) {
            return index == runtime.side.active_index()
                && shown != super::side_group::GroupsShown::Tabs;
        }
        if !shown.draws_tabs() {
            return false;
        }
        let Some(tab) = runtime.tabs.active_tab() else {
            return false;
        };
        if !tab.layout.contains(pane_id) {
            return false;
        }
        // A zoomed pane of this tab hides its other panes.
        match self.zoom_scope() {
            super::side_ops::ZoomScope::TabPane(zoomed) => zoomed == pane_id,
            _ => true,
        }
    }
}
