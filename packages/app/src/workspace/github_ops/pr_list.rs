//! The answers this window's pull requests page holds, for every tab, kept
//! by the process-wide [`crate::github::PrList`]. Answers change it only
//! through [`Workspace::apply_pr_list`]; the page's own moves are show, hide
//! and refresh. Which tab is shown is the page's alone
//! ([`crate::workspace::pages::pull_requests::PullRequestsPage`]). Not
//! persisted: a window starts with the page hidden.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use daruda_github::{PrFilter, RepoId};
use daruda_store::project::{LaneRef, ProjectId};
use gpui::{Context, Task};

use crate::github::{PrList, PrListAnswer, PrListUpdate};
use crate::workspace::Workspace;

/// What is known about one host's pull requests under one filter.
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

/// How often a shown page repaints so "Updated 2m ago" keeps counting
/// while no answer arrives to repaint it. The label steps in minutes.
const AGE_REPAINT: Duration = Duration::from_secs(30);

/// Whether the page is on screen, and what it has for each filter and host.
/// Its contents change only through this module (show, hide and the
/// poller's updates), so holders elsewhere can read it and nothing more.
#[derive(Debug, Default)]
pub(crate) struct PrListCache {
    /// `None` while the page is hidden.
    shown: Option<Shown>,
}

#[derive(Debug)]
struct Shown {
    hosts: HashMap<(PrFilter, String), HostList>,
    /// Repaints the page every [`AGE_REPAINT`]; ends with this state.
    _aging: Task<()>,
}

impl PrListCache {
    pub(crate) fn is_shown(&self) -> bool {
        self.shown.is_some()
    }

    /// What `host` has under `filter`; `Loading` while hidden.
    pub(crate) fn host(&self, filter: PrFilter, host: &str) -> &HostList {
        self.shown
            .as_ref()
            .and_then(|shown| shown.hosts.get(&(filter, host.to_owned())))
            .unwrap_or(&LOADING)
    }

    /// A shown cache holding `hosts`, without a repaint task.
    #[cfg(test)]
    pub(crate) fn shown_with(hosts: HashMap<(PrFilter, String), HostList>) -> Self {
        Self {
            shown: Some(Shown {
                hosts,
                _aging: Task::ready(()),
            }),
        }
    }

    /// When `host`'s last answer under `filter` arrived. It stays through a
    /// failed search, as the answer's rows do.
    pub(crate) fn answered_at(&self, filter: PrFilter, host: &str) -> Option<Instant> {
        self.host(filter, host).answer().map(|answer| answer.at)
    }

    /// Start holding answers; `false` when already shown.
    fn show(&mut self, aging: impl FnOnce() -> Task<()>) -> bool {
        if self.is_shown() {
            return false;
        }
        self.shown = Some(Shown {
            hosts: HashMap::new(),
            _aging: aging(),
        });
        true
    }

    fn hide(&mut self) -> bool {
        self.shown.take().is_some()
    }

    /// Apply what the poller says. Returns whether anything changed; a new
    /// arrival time alone does, since the header says how old the list is.
    fn apply(&mut self, update: PrListUpdate<'_>) -> bool {
        let Some(Shown { hosts, .. }) = &mut self.shown else {
            return false;
        };
        let (host, filter, list) = match update {
            PrListUpdate::Answer {
                host,
                filter,
                answer,
            } => (host, filter, HostList::Ready(answer.clone())),
            PrListUpdate::Cached {
                host,
                filter,
                answer,
            } => {
                let held = hosts.get(&(filter, host.to_owned())).unwrap_or(&LOADING);
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
                (host, filter, list)
            }
            PrListUpdate::Failed { host, filter } => {
                let last = hosts
                    .get(&(filter, host.to_owned()))
                    .and_then(HostList::answer)
                    .cloned();
                (host, filter, HostList::Failed { last })
            }
            PrListUpdate::HostBlocked { host } => {
                let before = hosts.len();
                hosts.retain(|(_, held), _| held != host);
                return hosts.len() != before;
            }
            PrListUpdate::Disabled => {
                let had = !hosts.is_empty();
                hosts.clear();
                return had;
            }
        };
        if hosts.get(&(filter, host.to_owned())) == Some(&list) {
            return false;
        }
        hosts.insert((filter, host.to_owned()), list);
        true
    }
}

impl Workspace {
    /// Discover every worktree's push target, even one whose panes never
    /// populated a lane-scoped Git cache. Every project, not the browsed
    /// one: a pull request is matched to its worktree in any project, and a
    /// worktree not read here would be offered to be made again.
    pub(in crate::workspace) fn refresh_pr_repositories(&mut self, cx: &mut Context<Self>) {
        self.refresh_push_targets(cx);
    }

    /// A project's worktrees were discovered: read just that project,
    /// leaving earlier discoveries alone.
    pub(in crate::workspace) fn refresh_pr_project_repositories(
        &mut self,
        project: ProjectId,
        cx: &mut Context<Self>,
    ) {
        let lanes: Vec<_> = self
            .project_for(project)
            .into_iter()
            .flat_map(|project| &project.lanes)
            .map(|lane| LaneRef {
                project,
                lane: lane.id,
            })
            .collect();
        for lane in lanes {
            self.refresh_push_target(lane, cx);
        }
    }

    /// GitHub repositories pushed to by lanes in the shared project scope.
    pub(crate) fn github_repos(&self) -> BTreeSet<RepoId> {
        self.lane_scoped
            .iter()
            .filter(|(lane, _)| self.page_scope_matches(lane.project))
            .filter_map(|(_, state)| state.github_push().map(|(repo, _)| repo))
            .collect()
    }

    /// Counts and selected rows share the process cache and its refresh
    /// cadence: every tab is searched while the page is on screen.
    pub(crate) fn pr_list_demands(&self) -> Vec<(PrFilter, BTreeSet<RepoId>)> {
        if !self.pr_list().is_shown() {
            return Vec::new();
        }
        let repos = self.github_repos();
        crate::workspace::pages::pull_requests::list::FILTERS
            .into_iter()
            .map(|filter| (filter, repos.clone()))
            .collect()
    }

    pub(crate) fn pr_list(&self) -> &PrListCache {
        &self.pages.pull_requests.cache
    }

    /// The page came on screen: hold answers for it until it leaves.
    pub(crate) fn show_pr_list(&mut self, cx: &mut Context<Self>) {
        let aging = || {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(AGE_REPAINT).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
        };
        if self.pages.pull_requests.cache.show(aging) {
            PrList::wanted(cx);
            cx.notify();
        }
    }

    /// The page left the screen: its hosts are no longer asked for it.
    pub(crate) fn hide_pr_list(&mut self, cx: &mut Context<Self>) {
        if self.pages.pull_requests.cache.hide() {
            cx.notify();
        }
    }

    /// Search this window's hosts again now, for every tab: the counts
    /// beside the shown one read the same cache and must not lag behind it.
    pub(crate) fn refresh_pr_list(&mut self, cx: &mut Context<Self>) {
        if !self.pr_list().is_shown() {
            return;
        }
        self.refresh_pr_repositories(cx);
        let hosts: BTreeSet<String> = self
            .github_repos()
            .iter()
            .map(|repo| repo.host().to_owned())
            .collect();
        self.refresh_pr_hosts(hosts, cx);
    }

    /// Search `host` alone again now, for every tab: its group's Try again,
    /// which must not spend a request on hosts that did not fail.
    pub(crate) fn refresh_pr_host(&mut self, host: &str, cx: &mut Context<Self>) {
        self.refresh_pr_hosts([host.to_owned()], cx);
    }

    fn refresh_pr_hosts(
        &mut self,
        hosts: impl IntoIterator<Item = String>,
        cx: &mut Context<Self>,
    ) {
        if self.pr_list().is_shown() {
            PrList::refresh(
                hosts,
                &crate::workspace::pages::pull_requests::list::FILTERS,
                cx,
            );
        }
    }

    /// The one write of the list the page shows.
    pub(crate) fn apply_pr_list(&mut self, update: PrListUpdate<'_>, cx: &mut Context<Self>) {
        if self.pages.pull_requests.cache.apply(update) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests;
