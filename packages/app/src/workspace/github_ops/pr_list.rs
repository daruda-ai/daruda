//! What this window's pull requests page shows, kept by the process-wide
//! [`crate::github::PrList`]. Answers change it only through
//! [`Workspace::apply_pr_list`]; the page's own moves are show, hide and
//! refresh. Not persisted: a window starts with the page hidden.

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use daruda_github::{PrFilter, RepoId};
use gpui::Context;

use crate::github::{PrList, PrListAnswer, PrListUpdate};
use crate::workspace::Workspace;

/// What is known about one host's pull requests under the shown filter.
/// Problems with `gh` itself (missing, logged out, rate limited) are the
/// host's availability, read from [`crate::github::PrPoller::availability`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum HostList {
    /// Not answered yet for this filter.
    #[default]
    Loading,
    /// The process's answer as it arrived, shared with every window showing
    /// the filter. It is not cut down to this window's repositories: the
    /// page reads each of its repositories out of it, and one the answer
    /// did not cover is still loading.
    Ready(PrListAnswer),
    /// The last search failed; `last` is what was shown before it.
    Failed { last: Option<PrListAnswer> },
}

impl HostList {
    /// The answer on show, which stays through a failed search.
    pub(crate) fn answer(&self) -> Option<&PrListAnswer> {
        match self {
            Self::Loading => None,
            Self::Ready(answer) => Some(answer),
            Self::Failed { last } => last.as_ref(),
        }
    }

    /// Whether the process's last answer has news for a window holding this:
    /// it is newer, or it covers a repository this one does not.
    fn lacks(&self, cached: &PrListAnswer) -> bool {
        self.answer()
            .is_none_or(|held| cached.at > held.at || !cached.covered.is_subset(&held.covered))
    }
}

static LOADING: HostList = HostList::Loading;

/// Whether the page is on screen, and what it has for each host.
#[derive(Debug, Default)]
pub(crate) enum PrListState {
    #[default]
    Hidden,
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
    pub(crate) fn host(&self, host: &str) -> &HostList {
        match self {
            Self::Hidden => &LOADING,
            Self::Shown { hosts, .. } => hosts.get(host).unwrap_or(&LOADING),
        }
    }

    /// When `host`'s last answer under the shown filter arrived. It stays
    /// through a failed search, as the answer's rows do.
    pub(crate) fn answered_at(&self, host: &str) -> Option<Instant> {
        self.host(host).answer().map(|answer| answer.at)
    }

    /// Show `filter`; another filter starts over. Returns whether it changed.
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

    fn hide(&mut self) -> bool {
        !matches!(std::mem::take(self), Self::Hidden)
    }

    /// Apply what the poller says. Returns whether anything changed; a new
    /// arrival time alone does, since the header says how old the list is.
    fn apply(&mut self, update: PrListUpdate<'_>) -> bool {
        let Self::Shown {
            filter: shown,
            hosts,
        } = self
        else {
            return false;
        };
        let (host, list) = match update {
            PrListUpdate::Answer {
                host,
                filter,
                answer,
            } if filter == *shown => (host, HostList::Ready(answer.clone())),
            PrListUpdate::Cached {
                host,
                filter,
                answer,
            } if filter == *shown => {
                let held = hosts.get(host).unwrap_or(&LOADING);
                if !held.lacks(answer) {
                    return false;
                }
                // The failure came after this answer: it stays said, over
                // rows that now cover what the window shows.
                let list = match held {
                    HostList::Failed { .. } => HostList::Failed {
                        last: Some(answer.clone()),
                    },
                    HostList::Loading | HostList::Ready(_) => HostList::Ready(answer.clone()),
                };
                (host, list)
            }
            PrListUpdate::Failed { host, filter } if filter == *shown => {
                let last = hosts.get(host).and_then(HostList::answer).cloned();
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
        if hosts.get(host) == Some(&list) {
            return false;
        }
        hosts.insert(host.to_owned(), list);
        true
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

    pub(crate) fn pr_list(&self) -> &PrListState {
        &self.pr_list
    }

    /// The page is on screen with `filter`, or switched to it.
    pub(crate) fn show_pr_list(&mut self, filter: PrFilter, cx: &mut Context<Self>) {
        if self.pr_list.show(filter) {
            PrList::wanted(cx);
            cx.notify();
        }
    }

    /// The page left the screen: its hosts are no longer asked for it.
    pub(crate) fn hide_pr_list(&mut self, cx: &mut Context<Self>) {
        if self.pr_list.hide() {
            cx.notify();
        }
    }

    /// Search this window's hosts again now, for the filter shown.
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
        if self.pr_list.apply(update) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests;
