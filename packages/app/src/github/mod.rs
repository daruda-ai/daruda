//! Lanes' GitHub pull requests, kept current for every window at once, and
//! the pull request list a page shows.

mod list;
mod list_schedule;
mod poller;
mod schedule;

pub(crate) use list::PrList;
pub(crate) use poller::PrPoller;
pub(crate) use schedule::is_unusable;

use daruda_github::availability::GhAvailability;
use daruda_github::query::RepoPrs;
use daruda_github::{PrFilter, PrSearch, PrSummary, RepoId};
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

/// One lane's question for the poller: which PR is `branch` in `repo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrDemand {
    pub repo: RepoId,
    pub branch: String,
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

/// What the list poller tells every window; a window applies only what
/// matches the filter its page shows.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PrListUpdate<'a> {
    /// A fresh answer for `host`'s repositories, the union over every window
    /// showing `filter`.
    Answer {
        host: &'a str,
        filter: PrFilter,
        search: &'a PrSearch,
    },
    /// The last answer, for a window that has none for `host` yet.
    Cached {
        host: &'a str,
        filter: PrFilter,
        search: &'a PrSearch,
    },
    /// The search failed; the last answer stays on show.
    Failed { host: &'a str, filter: PrFilter },
    /// `gh` on `host` is gone or logged out: drop what was read there.
    HostBlocked { host: &'a str },
    /// The feature was switched off: forget every list.
    Disabled,
}
