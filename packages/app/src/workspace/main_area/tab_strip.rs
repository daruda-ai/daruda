//! A worktree's tabs as one strip: the tabs in order, the one shown, and
//! the ones shown before it. No GPUI dependency.
//!
//! The strip owns the order and which tab is shown; a tab's own contents
//! (its split tree, the pane last used in it, its name) are the tab's and
//! are reached through [`TabStrip::tab_mut`]. The side tabs beside these
//! are [`super::side_group::SideGroup`], with the same words for the same
//! moves.

use super::pane::TabEntry;

/// A tab's identity in its worktree, among its tabs or its side tabs.
///
/// A type of its own, so a pane's id, a lane's id or an index is not taken
/// for one. Minted with pane ids out of one counter, from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::workspace) struct TabId(pub(in crate::workspace) u64);

/// Whether showing a tab remembers the one shown before it, which a close
/// goes back to. Drag previews and their unwind pass `Skip`: they are
/// look-aheads, and remembering them would let an abandoned drag redirect a
/// later close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum TabHistory {
    Record,
    Skip,
}

#[derive(Default)]
pub(in crate::workspace) struct TabStrip {
    tabs: Vec<TabEntry>,
    /// The tab shown; 0 when there is none.
    active: usize,
    /// Tabs shown before the one shown now, most recent last, each once and
    /// never the one shown. By id, so moving a tab leaves it as it is.
    recent: Vec<TabId>,
}

impl TabStrip {
    /// A strip as it was saved. An index past the tabs that came back shows
    /// the last of them.
    pub(in crate::workspace) fn restored(tabs: Vec<TabEntry>, active: usize) -> Self {
        Self {
            active: active.min(tabs.len().saturating_sub(1)),
            tabs,
            recent: Vec::new(),
        }
    }

    pub(in crate::workspace) fn active_index(&self) -> usize {
        self.active
    }

    pub(in crate::workspace) fn active_tab(&self) -> Option<&TabEntry> {
        self.tabs.get(self.active)
    }

    pub(in crate::workspace) fn active_tab_mut(&mut self) -> Option<&mut TabEntry> {
        self.tabs.get_mut(self.active)
    }

    /// The tab at `index`, to change what is the tab's own. Its place in
    /// the strip changes through the strip.
    pub(in crate::workspace) fn tab_mut(&mut self, index: usize) -> Option<&mut TabEntry> {
        self.tabs.get_mut(index)
    }

    pub(in crate::workspace) fn iter_mut(&mut self) -> std::slice::IterMut<'_, TabEntry> {
        self.tabs.iter_mut()
    }

    pub(in crate::workspace) fn position_of_id(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    /// The tabs shown before this one, most recent last.
    #[cfg(test)]
    pub(in crate::workspace) fn recent(&self) -> &[TabId] {
        &self.recent
    }

    /// Add `tab` at the end. The tab shown stays shown; the first tab of a
    /// strip is the one shown.
    pub(in crate::workspace) fn push(&mut self, tab: TabEntry) {
        self.tabs.push(tab);
    }

    /// Add `tab` at the end and show it, remembering the one shown before.
    pub(in crate::workspace) fn open(&mut self, tab: TabEntry) -> usize {
        self.tabs.push(tab);
        self.show(self.tabs.len() - 1, TabHistory::Record);
        self.active
    }

    /// Put `tab` at `at`, or at the end when `at` is past it. The tab shown
    /// stays shown.
    pub(in crate::workspace) fn insert(&mut self, at: usize, tab: TabEntry) -> usize {
        let at = at.min(self.tabs.len());
        let had_tabs = !self.tabs.is_empty();
        self.tabs.insert(at, tab);
        if had_tabs && at <= self.active {
            self.active += 1;
        }
        at
    }

    /// Take the tab at `index` out. When it was the one shown, the tab
    /// shown most recently before it takes its place, else the one that
    /// moved into its slot, else the one before.
    pub(in crate::workspace) fn remove(&mut self, index: usize) -> Option<TabEntry> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        self.recent.retain(|id| *id != tab.id);
        if self.tabs.is_empty() {
            self.active = 0;
            self.recent.clear();
        } else if index == self.active {
            let back = self.recent.pop().and_then(|id| self.position_of_id(id));
            self.active = back.unwrap_or_else(|| index.min(self.tabs.len() - 1));
        } else if index < self.active {
            self.active -= 1;
        }
        Some(tab)
    }

    /// Show the tab at `index`. `false` when there is none or it is shown.
    pub(in crate::workspace) fn activate(&mut self, index: usize, history: TabHistory) -> bool {
        if index >= self.tabs.len() || index == self.active {
            return false;
        }
        self.show(index, history);
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
        self.active = self.position_of_id(shown).unwrap_or(0);
        true
    }

    /// The one writer of `active` for a tab that is there.
    fn show(&mut self, index: usize, history: TabHistory) {
        let leaving = (index != self.active)
            .then(|| self.tabs.get(self.active))
            .flatten()
            .map(|tab| tab.id);
        self.active = index;
        let shown = self.tabs[index].id;
        self.recent.retain(|id| *id != shown);
        if let (TabHistory::Record, Some(leaving)) = (history, leaving) {
            self.recent.retain(|id| *id != leaving);
            self.recent.push(leaving);
        }
    }
}

/// Reading the strip is reading its tabs in order.
impl std::ops::Deref for TabStrip {
    type Target = [TabEntry];

    fn deref(&self) -> &[TabEntry] {
        &self.tabs
    }
}

impl<'a> IntoIterator for &'a TabStrip {
    type Item = &'a TabEntry;
    type IntoIter = std::slice::Iter<'a, TabEntry>;

    fn into_iter(self) -> Self::IntoIter {
        self.tabs.iter()
    }
}

/// Tests name tabs by number; outside tests a tab is compared with a tab.
#[cfg(test)]
impl PartialEq<u64> for TabId {
    fn eq(&self, other: &u64) -> bool {
        self.0 == *other
    }
}

#[cfg(test)]
impl From<u64> for TabId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

#[cfg(test)]
mod tests;
