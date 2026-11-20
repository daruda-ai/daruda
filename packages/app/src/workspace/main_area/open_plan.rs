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
    /// The tab shown on the left holds work: a shell or a chat, not files
    /// alone.
    pub(in crate::workspace) work_shown: bool,
    /// The width holds both groups at their least widths.
    pub(in crate::workspace) fits_both: bool,
}

impl GroupFacts {
    /// Whether a file opened goes to the side tabs: where the keyboard
    /// already is, or where the setting sends it. The side tabs are for
    /// reading beside work, so with files alone shown on the left, or no
    /// tab there, it opens among the tabs.
    pub(in crate::workspace) fn goes_beside(&self, entry: PaneEntry) -> bool {
        self.beside(entry, self.work_shown)
    }

    /// The same for what only the side tabs hold, a pull request: any tab
    /// on the left is enough to be beside.
    pub(in crate::workspace) fn side_only_goes_beside(&self, entry: PaneEntry) -> bool {
        self.beside(entry, self.tabs > 0)
    }

    /// Where only one group fits, an open that leaves the keyboard where
    /// it is would land in the group not drawn, so only one that enters
    /// goes aside.
    fn beside(&self, entry: PaneEntry, company: bool) -> bool {
        if self.keyboard_aside {
            return true;
        }
        let seen = entry == PaneEntry::Enter || self.fits_both;
        self.beside_setting && company && seen
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
    /// A new tab, in the side tabs when `beside`.
    New { beside: bool },
    /// A new tab, after the scratch tab left in the other group is closed:
    /// one preview is on screen at a time, and none is left behind as a tab
    /// nobody chose to keep. Which group the new tab joins is asked again
    /// once the scratch tab is gone, since closing it may take a tab, or
    /// the keyboard, with it.
    Replace(Slot),
}

/// Decide what opening as `intent` does, given `facts`.
pub(in crate::workspace) fn plan_open(intent: OpenIntent, facts: &OpenFacts) -> OpenPlan {
    if let Some(existing) = facts.existing {
        return OpenPlan::Show(existing);
    }
    let beside = facts.groups.goes_beside(intent.pane_entry());
    match facts.scratch {
        Some(scratch) if matches!(scratch.0, PanePlace::Side(_)) == beside => {
            OpenPlan::Refill(scratch)
        }
        // In the other group. Only a skim takes its scratch tab along; a
        // file opened to keep leaves the preview where it is.
        Some(scratch) if intent.claims_preview_slot() => OpenPlan::Replace(scratch),
        Some(_) | None => OpenPlan::New { beside },
    }
}

#[cfg(test)]
mod tests;
