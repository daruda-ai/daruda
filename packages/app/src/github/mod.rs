//! Lanes' GitHub pull requests, kept current for every window at once, and
//! the pull request list a page shows.

mod drafts;
pub(crate) mod links;
mod list;
mod list_schedule;
mod poller;
mod refine;
mod schedule;

pub(crate) use drafts::{DraftKeeping, DraftStore};
pub(crate) use links::LinkStore;
pub(crate) use list::PrList;
pub(crate) use list_schedule::audience;
pub(crate) use poller::PrPoller;
pub(crate) use refine::{Facet, NO_REFINE, Refine, RefineProblem};
pub(crate) use schedule::is_unusable;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use daruda_github::RepoPrs;
use daruda_github::availability::GhAvailability;
use daruda_github::{PrFilter, PrSearch, PrSearchState, PrSummary, RepoId};
use gpui::App;

/// `[github] enabled`: whether lanes look up their pull requests at all.
pub(crate) fn enabled(cx: &App) -> bool {
    crate::settings_store::SettingsStore::global(cx)
        .user()
        .github
        .enabled
}

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
    /// Also a review lane whose PR GitHub has no record of: there is nothing
    /// to show, and it must not read as [`Self::NoPr`], which offers one.
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

/// What a lane's pull request is looked up by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrSubject {
    /// The branch the lane pushes to: GitHub is asked which PR it heads.
    Branch(String),
    /// A review lane names its PR, whose head is a fork's branch that the
    /// base repository cannot be asked about by name.
    Review(u64),
}

/// One lane's question for the poller: which PR in `repo` it works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrDemand {
    pub repo: RepoId,
    pub subject: PrSubject,
    /// The lane is the one its window shows, so it is polled more often.
    pub focused: bool,
}

/// What the poller tells every window about its lanes' pull requests.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PrUpdate<'a> {
    /// The answer for one repository, current or re-applied from the cache.
    Answer { repo: &'a RepoId, prs: &'a RepoPrs },
    /// What is known about `gh` on a host changed.
    Host {
        host: &'a str,
        availability: GhAvailability,
    },
    /// The feature was switched on: push targets are not kept while it is off.
    Enabled,
    /// The feature was switched off: forget every lane's PR.
    Disabled,
}

/// One list search's answer, shared by every window showing its filter:
/// its first page, and every later page read since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrListAnswer {
    /// Every asked repository's pull requests read so far, newest first;
    /// `truncated` while GitHub matched more than it holds.
    pub search: Arc<PrSearch>,
    /// The repositories the search asked about: the union over the windows
    /// showing the filter then. A repository outside it has no answer yet,
    /// which is not the same as having no pull requests.
    pub covered: Arc<BTreeSet<RepoId>>,
    /// When its first page arrived, on the executor's clock.
    pub at: Instant,
    /// A later page can be read.
    pub more: bool,
    /// How many pull requests GitHub matched across the covered
    /// repositories, read or not, as its first pages said.
    pub matches: u32,
}

impl PrListAnswer {
    /// The search, if it asked about `repo`.
    pub(crate) fn covering(&self, repo: &RepoId) -> Option<&PrSearch> {
        self.covered.contains(repo).then_some(&*self.search)
    }
}

/// What the list poller tells every window; a window applies only what
/// matches the filter its page shows.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PrListUpdate<'a> {
    /// A fresh answer for `host` under `filter`.
    Answer {
        host: &'a str,
        filter: PrFilter,
        state: PrSearchState,
        refine: &'a Refine,
        answer: &'a PrListAnswer,
    },
    /// The process's last answer, for a window that has not seen it: one
    /// that started showing `filter` after it arrived.
    Cached {
        host: &'a str,
        filter: PrFilter,
        state: PrSearchState,
        refine: &'a Refine,
        answer: &'a PrListAnswer,
    },
    /// The search failed; the last answer stays on show.
    Failed {
        host: &'a str,
        filter: PrFilter,
        state: PrSearchState,
        refine: &'a Refine,
    },
    /// What was read on `host` can no longer be shown: `gh` is gone or
    /// logged out, or acts as another account now.
    HostBlocked { host: &'a str },
    /// The feature was switched off: forget every list.
    Disabled,
}
