//! What a pull request, as its detail was read, lets the user do to it on
//! GitHub: merge (and with which methods), auto-merge, mark ready for
//! review, close or reopen, and rerun failed checks. Plain decisions over [`PrDetail`]; asking GitHub is
//! [`crate::Gh`]'s.

use std::collections::BTreeSet;

use crate::{BlockReason, CheckStatus, MergeMethod, Mergeability, PrDetail, PrState};

/// Why merging is off. Anything else GitHub would refuse (a missing review,
/// a branch behind its base, a state not worked out yet) is left to GitHub
/// to say: the reason shows beside the pull request, and a refusal comes
/// back as the merge's error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeBlock {
    /// The repository allows no merge method to this viewer.
    NoMethods,
    /// Already in the merge queue: accepted, not merged yet.
    Queued,
    /// A draft is made ready for review first.
    Draft,
    Conflicting,
}

/// The order merge methods are offered in when GitHub preselects none.
const FALLBACK_ORDER: [MergeMethod; 3] =
    [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase];

impl PrDetail {
    /// Whether this is a draft. GitHub says so twice, as the pull request's
    /// own flag and as why it cannot merge, and the two can disagree for a
    /// moment; either one makes it a draft, so merging, marking ready and
    /// the state shown never decide differently.
    pub fn is_draft(&self) -> bool {
        match self.summary.state {
            PrState::Draft => true,
            PrState::Open => self.mergeability == Mergeability::Blocked(BlockReason::Draft),
            PrState::Merged | PrState::Closed => false,
        }
    }

    /// Why it cannot be merged at all; `None` when GitHub may be asked.
    pub fn merge_block(&self) -> Option<MergeBlock> {
        if self.methods.is_empty() {
            Some(MergeBlock::NoMethods)
        } else if self.queue.is_some() {
            Some(MergeBlock::Queued)
        } else if self.is_draft() {
            Some(MergeBlock::Draft)
        } else if self.mergeability == Mergeability::Conflicting {
            Some(MergeBlock::Conflicting)
        } else {
            None
        }
    }

    /// Whether closing may be asked of GitHub: open or draft, and the
    /// viewer may close it.
    pub fn can_close_now(&self) -> bool {
        self.summary.state.is_active() && self.can_close
    }

    /// Whether reopening may be asked of GitHub: closed unmerged, and the
    /// viewer may reopen it. GitHub can still refuse (a deleted head
    /// branch); that comes back as the reopen's error.
    pub fn can_reopen_now(&self) -> bool {
        self.summary.state == PrState::Closed && self.can_reopen
    }

    /// The methods auto-merge may be turned on with: open (not a draft),
    /// waiting on something (not conflicting, not ready to merge now),
    /// permitted, and not already on or queued. Empty otherwise; the order
    /// is [`Self::merge_menu`]'s.
    pub fn auto_merge_menu(&self) -> Vec<MergeMethod> {
        // ASSUMPTION: not measured — GitHub refuses auto-merge on a pull
        // request that could merge now ("clean status"); merge it instead.
        let possible = self.summary.state.is_active()
            && !self.is_draft()
            && !matches!(
                self.mergeability,
                Mergeability::Conflicting | Mergeability::Ready
            )
            && self.can_enable_auto_merge
            && self.auto_merge.is_none()
            && self.queue.is_none();
        if possible {
            self.merge_menu()
        } else {
            Vec::new()
        }
    }

    /// Whether auto-merge with `method` may be asked of GitHub.
    pub fn can_enable_auto_merge_with(&self, method: MergeMethod) -> bool {
        self.auto_merge_menu().contains(&method)
    }

    /// Whether turning auto-merge off may be asked of GitHub.
    pub fn can_disable_auto_merge_now(&self) -> bool {
        self.summary.state.is_active() && self.auto_merge.is_some() && self.can_disable_auto_merge
    }

    /// Whether merging with `method` may be asked of GitHub.
    pub fn can_merge(&self, method: MergeMethod) -> bool {
        self.merge_block().is_none() && self.methods.contains(&method)
    }

    /// The merge methods to offer: only those the repository allows,
    /// GitHub's preselected one for the viewer first. Without one (or one no
    /// longer allowed) a fixed order stands in: Squash, Merge, Rebase.
    pub fn merge_menu(&self) -> Vec<MergeMethod> {
        let default = self.default_method;
        let mut menu: Vec<MergeMethod> = default
            .into_iter()
            .chain(FALLBACK_ORDER.into_iter().filter(|m| Some(*m) != default))
            .filter(|method| self.methods.contains(method))
            .collect();
        menu.dedup();
        menu
    }

    /// Each Actions run with a failed job, once, in run id order: `gh run
    /// rerun --failed` reruns every failed job of a run, so two failed jobs
    /// of one run are one rerun.
    pub fn failed_runs(&self) -> Vec<u64> {
        let runs: BTreeSet<u64> = self
            .checks
            .iter()
            .filter(|entry| entry.run.status == CheckStatus::Failure)
            .filter_map(|entry| entry.job.map(|job| job.run))
            .collect();
        runs.into_iter().collect()
    }
}
