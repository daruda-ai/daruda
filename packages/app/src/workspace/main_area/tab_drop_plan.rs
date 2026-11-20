//! Where a dragged tab lands: the decision, apart from carrying it out.
//! No GPUI dependency.
//!
//! A tab is dragged from one of a worktree's two strips and dropped on a
//! slot of either, on a pane of the tabs, or on what the side tabs show.
//! The same answer draws the insertion line and carries out the drop, so a
//! drop that would be refused shows nothing.

use super::tab_strip::TabId;

/// One of a worktree's two tab strips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Strip {
    /// The tabs, each a split tree.
    Tabs,
    /// The side tabs beside them.
    Side,
}

/// The slot a drag is over: `index` of `strip`, as the cell `owner` saw it.
/// Tagged with the cell so only that cell lets go of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) struct DropSlot {
    pub(in crate::workspace) strip: Strip,
    pub(in crate::workspace) owner: TabId,
    /// The tab is put before the one at this index; the strip's length
    /// puts it last.
    pub(in crate::workspace) index: usize,
}

/// What a tab is let go over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum DropTarget {
    /// Before the tab at this index of a strip.
    Slot(Strip, usize),
    /// A strip, clear of its slots: after its last tab.
    End(Strip),
    /// A half of a pane of the tab on show.
    PaneHalf,
    /// What the side tabs show.
    SideContent,
}

/// The tab being dragged: where it sits, and whether its kind lets it sit
/// in the other strip (a tab of one file can go aside, a file can come
/// back, a pull request stays aside).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) struct Dragged {
    pub(in crate::workspace) from: Strip,
    pub(in crate::workspace) index: usize,
    pub(in crate::workspace) can_cross: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum TabDropPlan {
    /// Move the tab from `from` to `to` within `strip`.
    Reorder {
        strip: Strip,
        from: usize,
        to: usize,
    },
    /// Send the tab at `tab` to the side tabs, before `at`; `None` is last.
    SendAside { tab: usize, at: Option<usize> },
    /// Bring the side tab at `side` among the tabs, before `at`.
    SendBack { side: usize, at: Option<usize> },
    /// Merge the tab at `tab` into the pane under the pointer.
    Merge { tab: usize },
    /// Bring the side tab at `side` back and merge it into that pane.
    MergeFromAside { side: usize },
    /// Nothing happens, and nothing is drawn to say it would.
    Refuse,
}

pub(in crate::workspace) fn plan_tab_drop(dragged: Dragged, target: DropTarget) -> TabDropPlan {
    let Dragged {
        from,
        index,
        can_cross,
    } = dragged;
    match (from, target) {
        (_, DropTarget::Slot(strip, slot)) if strip == from => {
            // The slot is counted with the dragged tab still in place.
            let to = if slot > index { slot - 1 } else { slot };
            if to == index {
                TabDropPlan::Refuse
            } else {
                TabDropPlan::Reorder {
                    strip,
                    from: index,
                    to,
                }
            }
        }
        // Let go over its own strip but no slot of it: where it was.
        (_, DropTarget::End(strip)) if strip == from => TabDropPlan::Refuse,
        (Strip::Tabs, DropTarget::PaneHalf) => TabDropPlan::Merge { tab: index },
        (Strip::Side, DropTarget::SideContent) => TabDropPlan::Refuse,
        _ if !can_cross => TabDropPlan::Refuse,
        (Strip::Tabs, DropTarget::Slot(_, slot)) => TabDropPlan::SendAside {
            tab: index,
            at: Some(slot),
        },
        (Strip::Tabs, DropTarget::End(_) | DropTarget::SideContent) => TabDropPlan::SendAside {
            tab: index,
            at: None,
        },
        (Strip::Side, DropTarget::Slot(_, slot)) => TabDropPlan::SendBack {
            side: index,
            at: Some(slot),
        },
        (Strip::Side, DropTarget::End(_)) => TabDropPlan::SendBack {
            side: index,
            at: None,
        },
        (Strip::Side, DropTarget::PaneHalf) => TabDropPlan::MergeFromAside { side: index },
    }
}

#[cfg(test)]
mod tests;
