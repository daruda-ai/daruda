//! What this window's pull requests page shows, kept by the process-wide
//! [`crate::github::PrList`]. Answers change it only through
//! [`Workspace::apply_pr_list`]; the page's own moves are show, hide and
//! refresh. Not persisted: a window starts with the page hidden.

use std::collections::{BTreeSet, HashMap};

use daruda_github::{PrFilter, PrSearch, RepoId};
use gpui::Context;

use crate::github::{PrList, PrListUpdate};
use crate::workspace::Workspace;

/// What is known about one host's pull requests under the shown filter.
/// Problems with `gh` itself (missing, logged out, rate limited) are the
/// host's availability, read from [`crate::github::PrPoller::availability`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum HostList {
    /// Not answered yet for this filter.
    #[default]
    Loading,
    /// This window's repositories on the host, newest first.
    Ready(PrSearch),
    /// The last search failed; `last` is what was shown before it.
    Failed { last: Option<PrSearch> },
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the pull requests page calls it")
)]
static LOADING: HostList = HostList::Loading;

/// Whether the page is on screen, and what it has for each host.
#[derive(Debug, Default)]
pub(crate) enum PrListState {
    #[default]
    Hidden,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    Shown {
        filter: PrFilter,
        hosts: HashMap<String, HostList>,
    },
}

impl PrListState {
    pub(crate) fn filter(&self) -> Option<PrFilter> {
        match self {
            Self::Hidden => None,
            Self::Shown { filter, .. } => Some(*filter),
        }
    }

    /// What `host` has under the shown filter; `Loading` while hidden.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn host(&self, host: &str) -> &HostList {
        match self {
            Self::Hidden => &LOADING,
            Self::Shown { hosts, .. } => hosts.get(host).unwrap_or(&LOADING),
        }
    }

    /// Show `filter`; another filter starts over. Returns whether it changed.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    fn show(&mut self, filter: PrFilter) -> bool {
        if self.filter() == Some(filter) {
            return false;
        }
        *self = Self::Shown {
            filter,
            hosts: HashMap::new(),
        };
        true
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    fn hide(&mut self) -> bool {
        !matches!(std::mem::take(self), Self::Hidden)
    }

    /// Apply what the poller says. `repos` are this window's repositories:
    /// an answer covers every window's, so it is narrowed to these.
    /// Returns whether anything changed.
    fn apply(&mut self, update: PrListUpdate<'_>, repos: &BTreeSet<RepoId>) -> bool {
        let Self::Shown {
            filter: shown,
            hosts,
        } = self
        else {
            return false;
        };
        let next = match update {
            PrListUpdate::Answer {
                host,
                filter,
                search,
            } if filter == *shown => (host, HostList::Ready(narrow(search, repos))),
            PrListUpdate::Cached {
                host,
                filter,
                search,
            } if filter == *shown
                && hosts
                    .get(host)
                    .is_none_or(|list| *list == HostList::Loading) =>
            {
                (host, HostList::Ready(narrow(search, repos)))
            }
            PrListUpdate::Failed { host, filter } if filter == *shown => {
                let last = match hosts.get(host) {
                    Some(HostList::Ready(search)) => Some(search.clone()),
                    Some(HostList::Failed { last }) => last.clone(),
                    Some(HostList::Loading) | None => None,
                };
                (host, HostList::Failed { last })
            }
            PrListUpdate::HostBlocked { host } => return hosts.remove(host).is_some(),
            PrListUpdate::Disabled => {
                let had = !hosts.is_empty();
                hosts.clear();
                return had;
            }
            PrListUpdate::Answer { .. }
            | PrListUpdate::Cached { .. }
            | PrListUpdate::Failed { .. } => return false,
        };
        let (host, list) = next;
        if hosts.get(host) == Some(&list) {
            return false;
        }
        hosts.insert(host.to_owned(), list);
        true
    }
}

/// The part of `search` in `repos`.
fn narrow(search: &PrSearch, repos: &BTreeSet<RepoId>) -> PrSearch {
    PrSearch {
        items: search
            .items
            .iter()
            .filter(|item| repos.contains(&item.repo))
            .cloned()
            .collect(),
        truncated: search.truncated,
    }
}

impl Workspace {
    /// Every GitHub repository this window's lanes push to.
    pub(crate) fn github_repos(&self) -> BTreeSet<RepoId> {
        self.lane_scoped
            .values()
            .filter_map(|state| state.github_push().map(|(repo, _)| repo))
            .collect()
    }

    /// The filter the page shows and the repositories to search, while it
    /// is on screen.
    pub(crate) fn pr_list_demand(&self) -> Option<(PrFilter, BTreeSet<RepoId>)> {
        Some((self.pr_list.filter()?, self.github_repos()))
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn pr_list(&self) -> &PrListState {
        &self.pr_list
    }

    /// The page is on screen with `filter`, or switched to it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn show_pr_list(&mut self, filter: PrFilter, cx: &mut Context<Self>) {
        if self.pr_list.show(filter) {
            PrList::wanted(cx);
            cx.notify();
        }
    }

    /// The page left the screen: its hosts are no longer asked for it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn hide_pr_list(&mut self, cx: &mut Context<Self>) {
        if self.pr_list.hide() {
            cx.notify();
        }
    }

    /// Search this window's hosts again now, for the filter shown.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn refresh_pr_list(&mut self, cx: &mut Context<Self>) {
        let Some(filter) = self.pr_list.filter() else {
            return;
        };
        let hosts: BTreeSet<String> = self
            .github_repos()
            .iter()
            .map(|repo| repo.host().to_owned())
            .collect();
        PrList::refresh(hosts, filter, cx);
    }

    /// The one write of the list the page shows.
    pub(crate) fn apply_pr_list(&mut self, update: PrListUpdate<'_>, cx: &mut Context<Self>) {
        if self.pr_list.filter().is_none() {
            return;
        }
        let repos = self.github_repos();
        if self.pr_list.apply(update, &repos) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests;
