//! A worktree's tabs as one strip. No GPUI dependency.

/// A tab's identity in its worktree, among its tabs or its side tabs.
///
/// A type of its own, so a pane's id, a lane's id or an index is not taken
/// for one. Minted with pane ids out of one counter, from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(in crate::workspace) struct TabId(pub(in crate::workspace) u64);

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
