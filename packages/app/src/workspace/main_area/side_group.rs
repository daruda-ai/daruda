//! A worktree's side tab group: the tabs shown to the right of its own,
//! so a file, a diff or a pull request stays in view beside the chat it
//! is read for. No GPUI dependency.
//!
//! One tab holds one pane; nothing splits here. The group is the
//! worktree's, not a tab's: switching tabs on the left leaves it as it is.
//! Its panes live in the runtime's `panes` with every other pane, and
//! never become the runtime's `focused_pane_id`, which stays a pane of the
//! active tab on the left. Which group the keyboard is in is the
//! runtime's [`KeyboardIn`].

use gpui::SharedString;

use super::pane_tree::{DIVIDER_PX, PaneId};

/// The share of the width the group opens with.
const DEFAULT_RATIO: f32 = 0.45;
/// How far a drag can take the share either way.
const MIN_RATIO: f32 = 0.15;
const MAX_RATIO: f32 = 0.85;

pub(in crate::workspace) struct SideTab {
    pub(in crate::workspace) id: crate::workspace::main_area::tab_strip::TabId,
    pub(in crate::workspace) pane: PaneId,
    pub(in crate::workspace) user_label: Option<SharedString>,
}

pub(in crate::workspace) struct SideGroup {
    tabs: Vec<SideTab>,
    active: usize,
    /// Its share of the width both groups are drawn in.
    ratio: f32,
    /// Hidden by the user while it still holds tabs.
    collapsed: bool,
}

impl Default for SideGroup {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            ratio: DEFAULT_RATIO,
            collapsed: false,
        }
    }
}

/// Which of a worktree's tab groups the keyboard is in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum KeyboardIn {
    /// The focused pane of the active tab.
    #[default]
    Tabs,
    /// The pane the side group shows.
    Side,
}

/// Where a pane sits in its worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PanePlace {
    /// In the split tree of the tab at this index.
    Tab(usize),
    /// The pane of the side tab at this index.
    Side(usize),
}

/// What of the two groups is drawn in a given width.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(in crate::workspace) enum GroupsShown {
    /// The tabs alone: the side group is empty or collapsed, or there is
    /// no room for both and the keyboard is on the left.
    #[default]
    Tabs,
    /// The side group alone: no room for both and the keyboard is in it.
    Side,
    /// Both, at these widths, a divider between them.
    Both { tabs: f32, side: f32 },
}

impl GroupsShown {
    /// The width the tabs on the left are drawn in, of `total`.
    pub(in crate::workspace) fn tabs_width(self, total: f32) -> f32 {
        match self {
            Self::Tabs | Self::Side => total,
            Self::Both { tabs, .. } => tabs,
        }
    }

    pub(in crate::workspace) fn draws_tabs(self) -> bool {
        !matches!(self, Self::Side)
    }
}

impl SideGroup {
    /// A group as it was saved. An index past the tabs that came back
    /// shows the last of them; a share out of range is brought into it.
    pub(in crate::workspace) fn restored(
        tabs: Vec<SideTab>,
        active: usize,
        ratio: f32,
        collapsed: bool,
    ) -> Self {
        let mut group = Self {
            active: active.min(tabs.len().saturating_sub(1)),
            tabs,
            collapsed,
            ..Self::default()
        };
        // A file without a share reads as zero: the opening share stands.
        if ratio > 0.0 {
            group.set_ratio(ratio);
        }
        group
    }

    pub(in crate::workspace) fn tabs(&self) -> &[SideTab] {
        &self.tabs
    }

    pub(in crate::workspace) fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub(in crate::workspace) fn active_index(&self) -> usize {
        self.active
    }

    pub(in crate::workspace) fn active_tab(&self) -> Option<&SideTab> {
        self.tabs.get(self.active)
    }

    /// The pane the group shows.
    pub(in crate::workspace) fn active_pane(&self) -> Option<PaneId> {
        self.active_tab().map(|tab| tab.pane)
    }

    pub(in crate::workspace) fn position_of(&self, pane: PaneId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.pane == pane)
    }

    /// Drop the name the user gave the tab at `index`: it was for the file
    /// the tab held before.
    pub(in crate::workspace) fn clear_label(&mut self, index: usize) {
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.user_label = None;
        }
    }

    /// The pane the group has on show, when it is not hidden: what the
    /// keyboard can be in.
    pub(in crate::workspace) fn shown_pane(&self) -> Option<PaneId> {
        (!self.collapsed).then(|| self.active_pane()).flatten()
    }

    pub(in crate::workspace) fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// Hide or show the group; its tabs stay open.
    pub(in crate::workspace) fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    pub(in crate::workspace) fn ratio(&self) -> f32 {
        self.ratio
    }

    pub(in crate::workspace) fn set_ratio(&mut self, ratio: f32) {
        if ratio.is_finite() {
            self.ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        }
    }

    /// Add `tab` at the end and show it: a tab added is a tab asked for,
    /// so a collapsed group opens.
    pub(in crate::workspace) fn open(&mut self, tab: SideTab) -> usize {
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.collapsed = false;
        self.active
    }

    /// Take the tab at `index` out. The tab after it becomes the one shown
    /// when it was.
    pub(in crate::workspace) fn remove(&mut self, index: usize) -> Option<SideTab> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if index < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        Some(tab)
    }

    /// Show the tab at `index`. `false` when there is none or it is shown.
    pub(in crate::workspace) fn activate(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || index == self.active {
            return false;
        }
        self.active = index;
        true
    }

    /// Move the tab at `from` to `to`; the tab shown stays shown.
    pub(in crate::workspace) fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return false;
        }
        let shown = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == shown)
            .unwrap_or(0);
        true
    }

    /// Whether `total` pixels hold both groups at `min_width` each.
    pub(in crate::workspace) fn fits_beside(total: f32, min_width: f32) -> bool {
        total - DIVIDER_PX >= 2.0 * min_width
    }

    /// The side group's width at `ratio`, of the `room` both share.
    fn side_width(room: f32, ratio: f32) -> f32 {
        room * ratio
    }

    /// The share a divider drag reaches: `start` moved by `dx` pixels to
    /// the right, in `total` pixels. The side group is on the right, so a
    /// move left widens it.
    pub(in crate::workspace) fn ratio_after_drag(start: f32, dx: f32, total: f32) -> Option<f32> {
        let room = total - DIVIDER_PX;
        (room > 0.0).then(|| start - dx / room)
    }

    /// What is drawn in `total` pixels, where either group keeps at least
    /// `min_width` while both are shown. Without room for both, the group
    /// the keyboard is in.
    pub(in crate::workspace) fn shown_in(
        &self,
        total: f32,
        min_width: f32,
        keyboard: KeyboardIn,
    ) -> GroupsShown {
        if self.tabs.is_empty() || self.collapsed {
            return GroupsShown::Tabs;
        }
        let room = total - DIVIDER_PX;
        if !Self::fits_beside(total, min_width) {
            return match keyboard {
                KeyboardIn::Side => GroupsShown::Side,
                KeyboardIn::Tabs => GroupsShown::Tabs,
            };
        }
        let side = Self::side_width(room, self.ratio).clamp(min_width, room - min_width);
        GroupsShown::Both {
            tabs: room - side,
            side,
        }
    }
}

#[cfg(test)]
mod tests;
