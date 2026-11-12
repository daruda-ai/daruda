//! Lanes' GitHub pull requests, kept current for every window at once.

mod poller;

pub(crate) use poller::PrPoller;

use daruda_github::{PrSummary, RepoId};

/// A lane's pull request and the repository it lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrLink {
    pub repo: RepoId,
    pub pr: PrSummary,
}
