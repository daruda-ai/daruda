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

/// What is known about a lane's pull request. "No answer yet" and "the
/// default branch, which has no PR of its own" differ from "GitHub says this
/// branch has none": only the last may offer to create one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum PrLookup {
    /// Not answered for the lane's current branch, or the answer was dropped.
    #[default]
    Unknown,
    /// The lane is on the repository's default branch.
    DefaultBranch,
    /// GitHub answered: this branch has no pull request.
    NoPr,
    /// Boxed: a summary with its failed checks dwarfs the other variants,
    /// and every lane's state holds one of these.
    Linked(Box<PrLink>),
}

impl PrLookup {
    pub(crate) fn link(&self) -> Option<&PrLink> {
        match self {
            Self::Linked(link) => Some(link.as_ref()),
            Self::Unknown | Self::DefaultBranch | Self::NoPr => None,
        }
    }
}
