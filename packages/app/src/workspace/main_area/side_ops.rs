//! The side tab group's operations: sending a tab there and back, showing,
//! closing and reordering its tabs, moving the keyboard between the two
//! groups, and dragging the divider between them.
//!
//! The model is `side_group.rs`. The rule the rest of the workspace relies
//! on is kept here: a side pane never becomes `focused_pane_id`. Which
//! group the keyboard is in is written in one place, `Workspace::focus_pane`.

use daruda_store::project::LaneRef;
use gpui::{Context, Window};

use super::pane::{Pane, TabEntry};
use super::pane_tree::{PaneId, PaneLayout};
use super::side_group::{GroupsShown, KeyboardIn, PanePlace, SideGroup, SideTab};
use crate::ui::theme;
use crate::workspace::Workspace;

/// The divider between the tab groups, held.
#[derive(Debug, Clone, Copy)]
pub(in crate::workspace) struct SideDividerDrag {
    /// The worktree whose groups it divides: a drag does not follow the
    /// user to another one.
    lane: LaneRef,
    /// Where the pointer took it, in window pixels.
    anchor: f32,
    /// The side group's share when it was taken.
    start_ratio: f32,
}

/// Whether the side group takes `pane`: what is read beside a chat, not
/// the chat or a shell.
pub(in crate::workspace) fn side_hosts(pane: &Pane) -> bool {
    pane.content.behavior().hosting().aside()
}

impl Workspace {
    /// How the active worktree's two tab groups share the width, whatever
    /// covers them now. Pane sizes follow this, so a zoom or a page coming
    /// and going resizes nothing.
    pub(in crate::workspace) fn groups_layout(&self) -> GroupsShown {
        let runtime = self.active_runtime();
        runtime.side.shown_in(
            self.main_area.groups_width,
            theme::TAB_GROUP_MIN_WIDTH,
            runtime.keyboard_in(),
        )
    }

    /// What of the two groups is drawn now. A page, an unavailable
    /// worktree and a zoomed pane each take the whole area.
    pub(in crate::workspace) fn groups_shown(&self) -> GroupsShown {
        if self.active_page().is_some() || self.active_lane_is_inaccessible() {
            return GroupsShown::Tabs;
        }
        let runtime = self.active_runtime();
        let zoomed = self.main_area.zoomed_pane_id.is_some_and(|zoomed| {
            runtime
                .tabs
                .active_tab()
                .is_some_and(|tab| tab.layout.contains(zoomed))
        });
        if zoomed {
            return GroupsShown::Tabs;
        }
        self.groups_layout()
    }

    /// The pane commands about "what I am looking at" act on: find, save,
    /// close, send to chat. The side group's while the keyboard is there
    /// and it is drawn, else the focused pane. Splitting, zooming and the
    /// bottom input stay with `focused_pane_id`.
    pub(in crate::workspace) fn keyboard_pane(&self) -> PaneId {
        let runtime = self.active_runtime();
        if self.groups_shown() == GroupsShown::Tabs {
            runtime.focused_pane_id
        } else {
            runtime.keyboard_pane()
        }
    }

    /// Whether the keyboard is in a side tab that is drawn.
    pub(in crate::workspace) fn keyboard_is_aside(&self) -> bool {
        self.active_runtime().keyboard_in() == KeyboardIn::Side
            && self.groups_shown() != GroupsShown::Tabs
    }

    /// Record which group the keyboard is in. The one writer of
    /// `LaneRuntime::keyboard`, called as a pane takes the keyboard. Where
    /// only one group fits, the one drawn follows it.
    pub(super) fn set_keyboard_in(
        &mut self,
        place: KeyboardIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_runtime().keyboard == place {
            return;
        }
        let layout = self.groups_layout();
        self.active_runtime_mut().keyboard = place;
        if self.groups_layout() != layout {
            self.resize_all_tabs(window, cx);
        }
        cx.notify();
    }

    /// The side group holds tabs the user has hidden.
    pub(in crate::workspace) fn side_tabs_hidden(&self) -> bool {
        let side = &self.active_runtime().side;
        !side.is_empty() && side.is_collapsed() && self.active_page().is_none()
    }

    /// Whether the tab at `tab_index` can be sent to the side group: one
    /// pane, of a kind the group takes.
    pub(in crate::workspace) fn tab_can_go_aside(&self, tab_index: usize) -> bool {
        self.pane_to_send_aside(tab_index).is_some()
    }

    /// The single pane of the tab at `tab_index`, when the side group
    /// takes it.
    fn pane_to_send_aside(&self, tab_index: usize) -> Option<PaneId> {
        let runtime = self.active_runtime();
        let tab = runtime.tabs.get(tab_index)?;
        let PaneLayout::Pane(pane_id) = tab.layout else {
            return None;
        };
        let hosted = runtime
            .panes
            .iter()
            .find(|pane| pane.id == pane_id)
            .is_some_and(side_hosts);
        (hosted && !self.is_orchestrator_tab(tab)).then_some(pane_id)
    }

    /// Send the tab at `tab_index` to the side group and move into it.
    pub(in crate::workspace) fn send_tab_aside(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self.pane_to_send_aside(tab_index) else {
            return;
        };
        let tab = self.unlist_tab(tab_index);
        let runtime = self.active_runtime_mut();
        if runtime.preview_tab_id == Some(tab.id) {
            runtime.preview_tab_id = None;
        }
        runtime.side.push(SideTab {
            id: tab.id,
            pane,
            user_label: tab.user_label,
        });
        // The tabs left behind keep a focused pane of their own.
        if let Some(focused) = self.active_tab_mut().map(|tab| tab.last_focused_pane) {
            self.set_focused_pane(focused, window, cx);
        }
        self.resize_all_tabs(window, cx);
        self.focus_side_group(window, cx);
        cx.notify();
    }

    /// Whether the side tab at `index` can sit among the tabs: its kind
    /// says (a file can; a pull request is read only beside them).
    pub(in crate::workspace) fn side_tab_can_go_back(&self, index: usize) -> bool {
        let runtime = self.active_runtime();
        runtime
            .side
            .tabs()
            .get(index)
            .and_then(|tab| runtime.panes.iter().find(|pane| pane.id == tab.pane))
            .is_some_and(|pane| pane.content.behavior().hosting().among_tabs())
    }

    /// Bring the side tab at `index` back among the tabs and move into it.
    pub(in crate::workspace) fn send_side_tab_back(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.side_tab_can_go_back(index) {
            return;
        }
        let Some(tab) = self.active_runtime_mut().side.remove(index) else {
            return;
        };
        let runtime = self.active_runtime_mut();
        // Moved by hand is kept: it is no longer the scratch tab.
        if runtime.preview_tab_id == Some(tab.id) {
            runtime.preview_tab_id = None;
        }
        runtime.tabs.open(TabEntry {
            id: tab.id,
            layout: PaneLayout::Pane(tab.pane),
            last_focused_pane: tab.pane,
            user_label: tab.user_label,
        });
        self.main_area.zoomed_pane_id = None;
        self.set_focused_pane(tab.pane, window, cx);
        self.bump_activity(tab.pane);
        self.resize_all_tabs(window, cx);
        self.focus_pane(tab.pane, window, cx);
        cx.notify();
    }

    /// Show the side tab at `index` and move into it.
    pub(in crate::workspace) fn activate_side_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.active_runtime().side.tabs().len() {
            return;
        }
        self.active_runtime_mut().side.activate(index);
        self.focus_side_group(window, cx);
        cx.notify();
    }

    /// Show the side tab at `index` without moving the keyboard: a row the
    /// cursor rests on in a panel, which keeps its arrow keys.
    pub(in crate::workspace) fn show_side_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let side = &mut self.active_runtime_mut().side;
        let shown = side.activate(index);
        let opened = side.is_collapsed();
        side.set_collapsed(false);
        if opened {
            self.resize_all_tabs(window, cx);
        }
        if shown || opened {
            cx.notify();
        }
    }

    /// Ask to close the side tab at `index`, through the same questions as
    /// any pane: work still running, then unsaved edits.
    pub(in crate::workspace) fn request_close_side_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .active_runtime()
            .side
            .tabs()
            .get(index)
            .map(|tab| tab.pane)
        {
            self.request_close_pane(pane, window, cx);
        }
    }

    /// Drop the side tab at `index` with its pane.
    pub(in crate::workspace) fn close_side_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_aside = self.keyboard_is_aside();
        let Some(tab) = self.active_runtime_mut().side.remove(index) else {
            return;
        };
        self.forget_panes(&[tab.pane], cx);
        super::file_view_pane::images::release_pane_images(
            &mut self.active_runtime_mut().panes,
            &[tab.pane],
            window,
            cx,
        );
        self.active_runtime_mut().panes.retain(|p| p.id != tab.pane);
        self.resize_all_tabs(window, cx);
        if was_aside {
            self.focus_side_group(window, cx);
        }
        cx.notify();
    }

    /// Move the side tab at `from` to `to` in its strip.
    pub(in crate::workspace) fn move_side_tab(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        if self.active_runtime_mut().side.move_tab(from, to) {
            cx.notify();
        }
    }

    /// Next/Previous Tab while the keyboard is in the side group: show the
    /// tab `by` places on, wrapping. `false` when the keyboard is not there.
    pub(in crate::workspace) fn step_side_tab(
        &mut self,
        by: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.keyboard_is_aside() {
            return false;
        }
        let side = &self.active_runtime().side;
        let count = side.tabs().len() as isize;
        let next = (side.active_index() as isize + by).rem_euclid(count) as usize;
        self.activate_side_tab(next, window, cx);
        true
    }

    /// Move Tab Left/Right while the keyboard is in the side group: the
    /// shown tab moves `by` places, stopping at either end. `false` when
    /// the keyboard is not there.
    pub(in crate::workspace) fn shift_side_tab(
        &mut self,
        by: isize,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.keyboard_is_aside() {
            return false;
        }
        let from = self.active_runtime().side.active_index();
        if let Some(to) = from.checked_add_signed(by) {
            self.mutate_durable(cx, |ws, cx| ws.move_side_tab(from, to, cx));
        }
        true
    }

    /// Put the keyboard in the side group's shown pane. With none on show,
    /// it goes to the focused pane on the left.
    pub(in crate::workspace) fn focus_side_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.active_runtime().side.shown_pane() {
            Some(pane) => {
                self.bump_activity(pane);
                self.focus_pane(pane, window, cx);
            }
            None => self.focus_tabs_group(window, cx),
        }
        cx.notify();
    }

    /// Put the keyboard back in the focused pane of the tabs on the left.
    /// With no tab there, it is still recorded as out of the side group.
    pub(in crate::workspace) fn focus_tabs_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_focused_pane() {
            let focused = self.active_runtime().focused_pane_id;
            self.focus_pane(focused, window, cx);
        } else {
            self.set_keyboard_in(KeyboardIn::Tabs, window, cx);
        }
        cx.notify();
    }

    /// Move the keyboard to the other tab group, when it has something.
    pub(in crate::workspace) fn focus_other_tab_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.keyboard_is_aside() {
            self.focus_tabs_group(window, cx);
        } else {
            self.focus_side_group(window, cx);
        }
    }

    /// Send the tab the keyboard is in to the other group.
    pub(in crate::workspace) fn move_tab_to_other_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.keyboard_is_aside() {
            let index = self.active_runtime().side.active_index();
            self.send_side_tab_back(index, window, cx);
        } else {
            let index = self.active_runtime().tabs.active_index();
            self.send_tab_aside(index, window, cx);
        }
    }

    /// Hide the side group, or show it again; its tabs stay open. Shown,
    /// it takes the keyboard: where only one group fits, that is what
    /// brings it into view.
    pub(in crate::workspace) fn toggle_side_group(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_runtime().side.is_empty() {
            return;
        }
        let collapsed = !self.active_runtime().side.is_collapsed();
        self.active_runtime_mut().side.set_collapsed(collapsed);
        self.resize_all_tabs(window, cx);
        if collapsed {
            self.focus_tabs_group(window, cx);
        } else {
            self.focus_side_group(window, cx);
        }
        cx.notify();
    }

    /// The pointer took the divider between the groups at window `x`.
    pub(in crate::workspace) fn begin_side_divider_drag(&mut self, x: f32, cx: &mut Context<Self>) {
        self.main_area.side_divider_drag = Some(SideDividerDrag {
            lane: self.active,
            anchor: x,
            start_ratio: self.active_runtime().side.ratio(),
        });
        cx.notify();
    }

    /// The pointer moved to window `x` while holding the divider: the side
    /// group's share follows. Whether a drag was running.
    pub(in crate::workspace) fn drag_side_divider(
        &mut self,
        x: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.main_area.side_divider_drag else {
            return false;
        };
        // Another worktree came on screen under the pointer: let go.
        if drag.lane != self.active {
            self.end_side_divider_drag(cx);
            return false;
        }
        let Some(ratio) = SideGroup::ratio_after_drag(
            drag.start_ratio,
            x - drag.anchor,
            self.main_area.groups_width,
        ) else {
            return true;
        };
        let side = &mut self.active_runtime_mut().side;
        let before = side.ratio();
        side.set_ratio(ratio);
        if side.ratio() != before {
            self.resize_all_tabs(window, cx);
            cx.notify();
        }
        true
    }

    /// The divider was released: the share it reached is kept.
    pub(in crate::workspace) fn end_side_divider_drag(&mut self, cx: &mut Context<Self>) {
        if self.main_area.side_divider_drag.is_some() {
            self.mutate_durable(cx, |ws, _| ws.main_area.side_divider_drag = None);
            cx.notify();
        }
    }

    /// The side tab showing `path` as `source` reads it, if one is open.
    pub(in crate::workspace) fn side_tab_showing(
        &self,
        path: &std::path::Path,
        source: &super::file_view_pane::DiffSource,
    ) -> Option<usize> {
        let runtime = self.active_runtime();
        runtime.side.tabs().iter().position(|tab| {
            runtime
                .panes
                .iter()
                .find(|pane| pane.id == tab.pane)
                .and_then(|pane| pane.file_view())
                .is_some_and(|view| {
                    daruda_core::path::same_path(&view.path, path) && &view.source == source
                })
        })
    }

    /// Whether `pane` is a side tab's.
    pub(in crate::workspace) fn pane_is_aside(&self, pane: PaneId) -> bool {
        matches!(
            self.active_runtime().place_of(pane),
            Some(PanePlace::Side(_))
        )
    }
}
