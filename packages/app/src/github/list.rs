//! The pull request list, searched only for the filters a page shows. Driven
//! from [`PrPoller::tick`] so there is one loop, and asking only hosts the
//! poller's host state lets through, so a blocked host is not asked twice.

use std::collections::{BTreeSet, HashMap};

use daruda_github::{FailureKind, GhError, PrFilter, PrSearch, RepoId};
use gpui::{App, Global};

use super::PrListUpdate;
use super::list_schedule::{ListKey, ListOutcome, ListSchedule, ListWanted};
use super::poller::{PrPoller, log_failure};
use crate::gh_host::GhHost;
use crate::window_registry::WindowRegistry;

#[derive(Default)]
pub(crate) struct PrList {
    schedule: ListSchedule,
}

impl Global for PrList {}

impl PrList {
    /// A page started showing a list or changed its filter: run a round now
    /// rather than at the next tick. Deferred: the caller is inside a
    /// workspace update the round would re-enter.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn wanted(cx: &mut App) {
        cx.defer(PrPoller::tick);
    }

    /// Search `hosts` for `filter` again at the next round, whatever the
    /// cadence says: the user pressed refresh.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(crate) fn refresh(hosts: impl IntoIterator<Item = String>, filter: PrFilter, cx: &mut App) {
        let schedule = &mut cx.default_global::<Self>().schedule;
        for host in hosts {
            schedule.refresh(ListKey { host, filter });
        }
        cx.defer(PrPoller::tick);
    }

    /// One round, run by [`PrPoller::tick`] while the feature is on: gather
    /// what every window shows, then send each search that is due.
    pub(super) fn tick(cx: &mut App) {
        let mut wanted: ListWanted = HashMap::new();
        WindowRegistry::for_each_workspace(cx, |ws, _, _| {
            let Some((filter, repos)) = ws.pr_list_demand() else {
                return;
            };
            for repo in repos {
                let key = ListKey {
                    host: repo.host().to_owned(),
                    filter,
                };
                wanted.entry(key).or_default().insert(repo);
            }
        });

        Self::apply_cached(&wanted, cx);
        let now = cx.background_executor().now();
        let open: BTreeSet<String> = wanted
            .keys()
            .filter(|key| PrPoller::takes_requests(&key.host, now, cx))
            .map(|key| key.host.clone())
            .collect();
        let schedule = &mut cx.default_global::<Self>().schedule;
        schedule.forget_unwanted(&wanted);
        let due = schedule.begin(wanted, |host| open.contains(host), now);
        for (key, repos) in due {
            Self::search(key, repos, cx);
        }
    }

    /// The feature was switched off: forget every answer and every list.
    pub(super) fn disable(cx: &mut App) {
        cx.default_global::<Self>().schedule.forget_answers();
        Self::broadcast(PrListUpdate::Disabled, cx);
    }

    /// `gh` on `host` became unusable: drop what was read there.
    pub(super) fn host_blocked(host: &str, cx: &mut App) {
        cx.default_global::<Self>().schedule.forget_host(host);
        Self::broadcast(PrListUpdate::HostBlocked { host }, cx);
    }

    /// Give a window that just started showing a filter the last answers at
    /// once. The workspace keeps what it already has, so this repaints nothing
    /// in a round without news.
    fn apply_cached(wanted: &ListWanted, cx: &mut App) {
        let cached = cx.default_global::<Self>().schedule.cached(wanted);
        if cached.is_empty() {
            return;
        }
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| {
            for (key, search) in &cached {
                let update = PrListUpdate::Cached {
                    host: &key.host,
                    filter: key.filter,
                    search,
                };
                ws.apply_pr_list(update, cx);
            }
        });
    }

    fn search(key: ListKey, repos: Vec<RepoId>, cx: &mut App) {
        let gh = GhHost::global(cx);
        let (host, filter) = (key.host.clone(), key.filter);
        let task = cx
            .background_executor()
            .spawn(async move { gh.search_prs(&host, &repos, filter) });
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update(|cx| Self::finish(key, result, cx));
        })
        .detach();
    }

    fn finish(key: ListKey, result: Result<PrSearch, GhError>, cx: &mut App) {
        let now = cx.background_executor().now();
        let enabled = super::enabled(cx);
        let result = result.map_err(|e| {
            if enabled {
                log_failure("github.list", &key.host, &e);
            }
            e.kind()
        });
        let outcome = cx
            .default_global::<Self>()
            .schedule
            .finish(&key, result, enabled, now);
        let (host, filter) = (key.host.as_str(), key.filter);
        match outcome {
            ListOutcome::Answer(search) => {
                PrPoller::host_answered(host, cx);
                let update = PrListUpdate::Answer {
                    host,
                    filter,
                    search: &search,
                };
                Self::broadcast(update, cx);
            }
            ListOutcome::Failed(kind) => {
                Self::broadcast(PrListUpdate::Failed { host, filter }, cx);
                if kind != FailureKind::Transient {
                    PrPoller::host_failed(host, kind, cx);
                }
            }
            ListOutcome::Dropped => {}
        }
    }

    fn broadcast(update: PrListUpdate<'_>, cx: &mut App) {
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| ws.apply_pr_list(update, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_store::SettingsStore;

    #[gpui::test]
    fn a_refresh_without_a_page_searches_nothing(cx: &mut gpui::TestAppContext) {
        let gh = crate::test_support::fake_gh(cx);
        cx.update(|cx| {
            SettingsStore::init(cx);
            PrList::refresh(["github.com".to_owned()], PrFilter::Mine, cx);
        });
        cx.run_until_parked();
        assert!(gh.calls().is_empty());
    }
}
