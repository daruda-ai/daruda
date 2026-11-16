//! What a pull request, as its detail was read, lets the user do to it on
//! GitHub: merge (and with which methods), mark ready for review, and rerun
//! failed checks. Plain decisions over [`PrDetail`]; asking GitHub is
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
        } else if self.is_draft() {
            Some(MergeBlock::Draft)
        } else if self.mergeability == Mergeability::Conflicting {
            Some(MergeBlock::Conflicting)
        } else {
            None
        }
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
