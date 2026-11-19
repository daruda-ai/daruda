//! Where something opened in a worktree goes, decided from plain facts:
//! which tab already shows it, which is the scratch tab, where the
//! keyboard is and what the width holds. No GPUI dependency and no
//! workspace: every rule is one function a table of cases can cover, and
//! the workspace carries the plan out.

use super::pane_tree::PaneId;
use super::side_group::PanePlace;
use super::tab_ops::{OpenIntent, PaneEntry};

/// What the two tab groups are like when something is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) struct GroupFacts {
    /// The keyboard is in a side tab that is drawn.
    pub(in crate::workspace) keyboard_aside: bool,
    /// `file_viewer.open_beside`.
    pub(in crate::workspace) beside_setting: bool,
    /// How many tabs the worktree has on the left.
    pub(in crate::workspace) tabs: usize,
    /// The width holds both groups at their least widths.
    pub(in crate::workspace) fits_both: bool,
}

impl GroupFacts {
    /// Whether what is opened goes to the side tabs: where the keyboard
    /// already is, or where the setting sends it. With no tab on the left
    /// there is nothing to open beside, so it opens there. Where only one
    /// group fits, an open that leaves the keyboard where it is would land
    /// in the group not drawn, so only one that enters goes aside.
    pub(in crate::workspace) fn goes_beside(&self, entry: PaneEntry) -> bool {
        if self.keyboard_aside {
            return true;
        }
        let seen = entry == PaneEntry::Enter || self.fits_both;
        self.beside_setting && self.tabs > 0 && seen
    }
}

/// A tab and the pane it holds.
pub(in crate::workspace) type Slot = (PanePlace, PaneId);

/// What an open finds in the worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) struct OpenFacts {
    pub(in crate::workspace) groups: GroupFacts,
    /// The tab already showing what is asked for, in either group.
    pub(in crate::workspace) existing: Option<Slot>,
    /// The scratch tab: the one a preview may replace. `None` in
    /// multi-tab mode, and while it holds unsaved edits.
    pub(in crate::workspace) scratch: Option<Slot>,
}

/// What an open does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum OpenPlan {
    /// It is open already: show that tab.
    Show(Slot),
    /// The scratch tab takes it.
    Refill(Slot),
    /// A new tab, in the side tabs when `beside`. `close` is a scratch tab
    /// left in the other group, which goes first: one preview is on screen
    /// at a time, and none is left behind as a tab nobody chose to keep.
    New { beside: bool, close: Option<Slot> },
}

/// Decide what opening as `intent` does, given `facts`.
pub(in crate::workspace) fn plan_open(intent: OpenIntent, facts: &OpenFacts) -> OpenPlan {
    if let Some(existing) = facts.existing {
        return OpenPlan::Show(existing);
    }
    let entry = intent.pane_entry();
    let beside = facts.groups.goes_beside(entry);
    match facts.scratch {
        Some(scratch) if matches!(scratch.0, PanePlace::Side(_)) == beside => {
            OpenPlan::Refill(scratch)
        }
        // In the other group. Only a skim takes its scratch tab along; a
        // file opened to keep leaves the preview where it is.
        Some(scratch) if intent.claims_preview_slot() => {
            // Closing a scratch tab that was the last tab on the left
            // leaves nothing to open beside.
            let tabs = match scratch.0 {
                PanePlace::Tab(_) => facts.groups.tabs.saturating_sub(1),
                PanePlace::Side(_) => facts.groups.tabs,
            };
            let after = GroupFacts {
                tabs,
                ..facts.groups
            };
            OpenPlan::New {
                beside: after.goes_beside(entry),
                close: Some(scratch),
            }
        }
        Some(_) | None => OpenPlan::New {
            beside,
            close: None,
        },
    }
}

#[cfg(test)]
mod tests;
