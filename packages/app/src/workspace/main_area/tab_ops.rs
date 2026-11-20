mod close_guard;
mod focus;
mod pane_structure;

use gpui::{Context, Window};

use super::pane::TabEntry;
use super::pane_tree::{PaneId, PaneLayout};
use super::tab_strip::TabHistory;
use crate::workspace::Workspace;
use crate::workspace::main_area::file_view_pane::images::release_pane_images;

/// What content a newly split-off pane should hold. Keeps the split entry
/// point free of a `bool`/`Option` flag pair (an invalid state would be
/// unrepresentable): each variant maps to exactly one pane constructor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum NewPaneKind {
    Terminal,
    AgentChat,
}

/// Whether activating a pane also moves the user into it.
///
/// Splitting this out is what keeps "put this file on screen" from carrying
/// "and start typing at it": [`Workspace::focus_pane`] surfaces the bottom
/// dock's Input panel and takes keyboard focus, which is right for a pane the
/// user walked into and wrong for one a left-dock row is previewing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::workspace) enum PaneEntry {
    /// Keyboard focus and the bottom input follow the pane.
    Enter,
    /// The tab and the focused pane change; the user stays where they are.
    Preview,
}

/// What a caller is doing when it opens a file, over the two axes that
/// differ: who ends up with keyboard focus, and whether the tab is the
/// replaceable preview slot. zed carries these as `focus_item` and
/// `allow_preview`; only three of the four combinations are real, so naming
/// them keeps the fourth from being expressible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::workspace) enum OpenIntent {
    /// A left-dock row the cursor is resting on: the file appears, the user
    /// stays in the panel, and the tab is the slot a later skim may reuse.
    Preview,
    /// A left-dock row the user pressed Enter on: the file keeps its tab, but
    /// the user still stays in the panel so the arrows keep working.
    Commit,
    /// Navigation into the file from anywhere else — a flow, an agent link, a
    /// skill: the pane takes focus and keeps its tab.
    Enter,
}

impl OpenIntent {
    /// Whether the user moves into the opened pane.
    pub(in crate::workspace) fn pane_entry(self) -> PaneEntry {
        match self {
            Self::Enter => PaneEntry::Enter,
            Self::Preview | Self::Commit => PaneEntry::Preview,
        }
    }

    /// Whether the opened tab becomes the replaceable preview slot.
    pub(in crate::workspace) fn claims_preview_slot(self) -> bool {
        matches!(self, Self::Preview)
    }
}

impl Workspace {
    pub(in crate::workspace) fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // An inaccessible active lane renders the empty-state; spawning a tab
        // would root a PTY at the dead lane path and escape it. No-op. (A
        // workspace with no project at all still allows tabs.)
        if self.active_lane_is_inaccessible() {
            return;
        }
        let pane = match self.create_pane(window, cx) {
            Ok(p) => p,
            Err(e) => {
                self.report_pane_error(
                    &crate::surface::strings::error::pane_context_new_tab(),
                    e,
                    cx,
                );
                return;
            }
        };
        let pane_id = pane.id;
        let tab_id = self.alloc_tab_id();

        self.active_runtime_mut().panes.push(pane);
        self.active_runtime_mut().tabs.open(TabEntry {
            id: tab_id,
            layout: PaneLayout::Pane(pane_id),
            last_focused_pane: pane_id,
            user_label: None,
        });
        self.set_focused_pane(pane_id, window, cx);
        self.bump_activity(pane_id);
        self.focus_pane(pane_id, window, cx);
        self.resize_all_tabs(window, cx);
        cx.notify();
    }

    /// Total open user tabs across every lane runtime in the window (all
    /// projects, all lanes), excluding the orchestrator's tab.
    #[cfg(test)]
    pub(in crate::workspace) fn total_open_tabs(&self) -> usize {
        self.main_area
            .runtimes
            .values()
            .flat_map(|rt| rt.tabs.iter())
            .filter(|tab| !self.is_orchestrator_tab(tab))
            .count()
    }

    /// Drop every pane in the active lane's runtime and reset it to an empty
    /// [`LaneRuntime`](crate::workspace::LaneRuntime) (the "no tabs" empty
    /// state). Releases PTY tracking, activity counters, zoom, and drafts for
    /// the removed panes first (same cleanup as `close_tab_at`). The lane
    /// stays in the sidebar.
    pub(in crate::workspace) fn empty_active_lane_runtime(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane_ids: Vec<PaneId> = self.active_runtime().panes.iter().map(|p| p.id).collect();
        if pane_ids
            .iter()
            .any(|&id| self.main_area.zoomed_pane() == Some(id))
        {
            self.main_area.set_zoomed_pane(None);
        }
        self.main_area.pane_drop_hover = None;
        self.forget_panes(&pane_ids, cx);
        release_pane_images(&mut self.active_runtime_mut().panes, &pane_ids, window, cx);
        *self.active_runtime_mut() = crate::workspace::LaneRuntime::default();
        // Emptying a lane is a durable change that must survive restart, so
        // self-schedule the persist here: interactive close paths reach
        // `close_tab_at` outside any `mutate_durable` wrapper. The `cx.defer`
        // coalescing makes a nested call from a wrapped caller harmless.
        self.mutate_durable(cx, |_, _| {});
    }

    pub(in crate::workspace) fn close_tab_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.active_runtime().tabs.len() {
            return;
        }
        if self.is_orchestrator_tab(&self.active_runtime().tabs[index]) {
            self.hide_orchestrator_tab(window, cx);
            return;
        }
        // The active lane's last tab empties the lane in place (→ empty-state)
        // and keeps the window, even when it is the window's last tab: on
        // Windows and Linux the last window closing quits the app. Tabs
        // still open beside it stay, with the empty state on the left.
        if self.active_runtime().tabs.len() <= 1 && self.active_runtime().side.is_empty() {
            self.empty_active_lane_runtime(window, cx);
            return;
        }

        let tab = self.unlist_tab(index);
        let pane_ids = tab.layout.pane_ids();
        if pane_ids
            .iter()
            .any(|&id| self.main_area.zoomed_pane() == Some(id))
        {
            self.main_area.set_zoomed_pane(None);
        }
        self.forget_panes(&pane_ids, cx);
        release_pane_images(&mut self.active_runtime_mut().panes, &pane_ids, window, cx);
        self.active_runtime_mut()
            .panes
            .retain(|p| !pane_ids.contains(&p.id));
        self.focus_active_tab(window, cx);
        cx.notify();
    }

    /// Take the tab at `index` out of the strip and settle which tab is
    /// shown after it. Its panes stay in the runtime: the caller drops them
    /// or gives them another place. The caller checked the index.
    pub(super) fn unlist_tab(&mut self, index: usize) -> TabEntry {
        let runtime = self.active_runtime_mut();
        let tab = runtime
            .tabs
            .remove(index)
            .expect("the caller checked the index");
        if runtime.tabs.is_empty() {
            runtime.focused_pane_id = PaneId::NONE;
        }
        tab
    }

    /// Make `pane` the tabs' focused pane once what was focused there is
    /// gone. The keyboard goes with it, unless it is in a side tab: a close
    /// among the tabs does not take the keyboard from what is read beside
    /// them.
    pub(in crate::workspace) fn refocus_tabs_after_close(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_focused_pane(pane, window, cx);
        if !self.keyboard_is_aside() {
            self.bump_activity(pane);
            self.focus_pane(pane, window, cx);
        }
    }

    /// Move into the pane the tab now shown was last used in. With no tab
    /// left, the keyboard goes to the tabs beside them, if any.
    pub(super) fn focus_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focused) = self
            .active_runtime()
            .tabs
            .active_tab()
            .map(|tab| tab.last_focused_pane)
        {
            self.refocus_tabs_after_close(focused, window, cx);
        } else {
            self.focus_side_group(window, cx);
        }
    }

    /// Shared core behind `activate_tab` and `switch_tab_for_drag_preview`:
    /// the switch itself and the zoom and drag hover it clears. Returns the
    /// new tab's `last_focused_pane` on a real switch, `None` on a no-op
    /// (out-of-range or already active): callers branch on that to decide
    /// whether to focus and persist.
    fn switch_active_tab_index(&mut self, index: usize, history: TabHistory) -> Option<PaneId> {
        if !self.active_runtime_mut().tabs.activate(index, history) {
            return None;
        }
        self.main_area.set_zoomed_pane(None);
        // Drop any in-flight drag hover so a stale half-fill overlay does
        // not linger on the newly-activated tab. The caller's notify covers it.
        self.main_area.pane_drop_hover = None;
        Some(self.active_runtime().tabs[index].last_focused_pane)
    }

    pub(in crate::workspace) fn activate_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_tab_as(index, PaneEntry::Enter, window, cx);
    }

    /// [`Self::activate_tab`], with `entry` deciding whether the user moves
    /// into the tab's pane or only sees it. See [`PaneEntry`].
    pub(in crate::workspace) fn activate_tab_as(
        &mut self,
        index: usize,
        entry: PaneEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focused) = self.switch_active_tab_index(index, TabHistory::Record) else {
            return;
        };
        // Switching tabs changes the focused pane — route through the
        // chokepoint so the bottom-dock draft swaps to the new tab's
        // pane. `set_focused_pane` also runs `refresh_pane_dimming`.
        self.set_focused_pane(focused, window, cx);
        self.mutate_durable_in(window, cx, |ws, window, cx| {
            ws.bump_activity(focused);
            if entry == PaneEntry::Enter {
                ws.focus_pane(focused, window, cx);
            }
        });
        cx.notify();
    }

    /// Tab-bar drag-reorder's hover-to-switch preview: swap the active tab
    /// without focusing the new tab's pane or persisting. No focus, no
    /// persist — the hover timer's `cx.update` re-entry only gives
    /// App-level access, not `&mut Window`, so `set_focused_pane` /
    /// `focus_pane` are unreachable here; and stealing OS keyboard focus
    /// mid-drag would be disruptive anyway. The switch is purely visual
    /// until `drop_tab_onto_bar` commits a real reorder through
    /// `mutate_durable`.
    pub(in crate::workspace) fn switch_tab_for_drag_preview(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        // Stash on the first hop only — later hops must not overwrite it, or
        // an abandoned drag would unwind to the last tab passed over instead
        // of where the drag started.
        let pre_drag = self.active_runtime().tabs.active_tab().map(|t| t.id);
        if self
            .switch_active_tab_index(index, TabHistory::Skip)
            .is_some()
        {
            if self.main_area.tab_preview_restore.is_none() {
                self.main_area.tab_preview_restore = pre_drag;
            }
            cx.notify();
        }
    }

    /// Re-activate a tab by id, for unwinding a drag preview. `false` when
    /// that tab is gone (closed mid-drag) or already active. Skips history
    /// for the same reason the preview hops do — the unwind is part of the
    /// look-ahead, not navigation the user performed.
    pub(super) fn restore_active_tab_by_id(
        &mut self,
        tab_id: crate::workspace::main_area::tab_strip::TabId,
    ) -> bool {
        let Some(index) = self
            .active_runtime()
            .tabs
            .iter()
            .position(|t| t.id == tab_id)
        else {
            return false;
        };
        self.switch_active_tab_index(index, TabHistory::Skip)
            .is_some()
    }

    pub(in crate::workspace) fn move_tab(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        if self.active_runtime_mut().tabs.move_tab(from, to) {
            cx.notify();
        }
    }

    pub(in crate::workspace) fn bump_activity(&mut self, pane_id: PaneId) {
        self.main_area.activity_tick += 1;
        self.main_area
            .activity_counter
            .insert(pane_id, self.main_area.activity_tick);
    }

    // ---- Context-menu tab operations ----
    //
    // Close Other Tabs / Close Tabs to Right route through
    // `request_close_tabs_bulk`, whose dirty-prompt covers every dirty
    // pane in the closing set. Never loop over `close_tab_at`
    // directly for a multi-tab close — that silently drops unsaved edits.

    pub(in crate::workspace) fn close_other_tabs(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices: Vec<usize> = (0..self.active_runtime().tabs.len())
            .rev()
            .filter(|&index| index != tab_index)
            .collect();
        self.request_close_tabs_bulk(indices, window, cx);
    }

    pub(in crate::workspace) fn close_tabs_to_right(
        &mut self,
        tab_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices: Vec<usize> = (tab_index + 1..self.active_runtime().tabs.len())
            .rev()
            .collect();
        self.request_close_tabs_bulk(indices, window, cx);
    }

    pub(in crate::workspace) fn move_tab_left(&mut self, tab_index: usize, cx: &mut Context<Self>) {
        if tab_index > 0 {
            self.move_tab(tab_index, tab_index - 1, cx);
        }
    }

    pub(in crate::workspace) fn move_tab_right(
        &mut self,
        tab_index: usize,
        cx: &mut Context<Self>,
    ) {
        if tab_index + 1 < self.active_runtime().tabs.len() {
            self.move_tab(tab_index, tab_index + 1, cx);
        }
    }

    // ---- Split management ----

    // ---- Dirty-checked close entry points ----
}
