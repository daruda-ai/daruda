//! The pull request list, searched only for the filters a page shows. Driven
//! from [`PrPoller::tick`] so there is one loop, and asking only hosts the
//! poller's host state lets through, so a blocked host is not asked twice.

use std::collections::{BTreeSet, HashMap};

use daruda_github::{FailureKind, GhError, PrAudience, PrSearchPage, PrSearchState, RepoId};
use gpui::{App, Global};

use super::PrListUpdate;
use super::list_schedule::{Due, ListKey, ListOutcome, ListSchedule, ListWanted};
use super::poller::{PrPoller, log_failure};
use super::refine::Refine;
use crate::gh_host::GhHost;
use crate::window_registry::WindowRegistry;

#[derive(Default)]
pub(crate) struct PrList {
    schedule: ListSchedule,
}

impl Global for PrList {}

impl PrList {
    /// Read the shared scheduler rather than maintaining a UI refresh flag.
    pub(crate) fn is_refreshing(
        filter: PrAudience,
        state: PrSearchState,
        refine: &Refine,
        repos: &BTreeSet<RepoId>,
        cx: &App,
    ) -> bool {
        let Some(list) = cx.try_global::<Self>() else {
            return false;
        };
        let now = cx.background_executor().now();
        repos.iter().any(|repo| {
            PrPoller::takes_requests(repo.host(), now, cx)
                && list.schedule.is_refreshing(&ListKey {
                    host: repo.host().to_owned(),
                    filter,
                    state,
                    refine: refine.clone(),
                })
        })
    }

    /// Whether a later page is being read for any of `repos`' hosts.
    pub(crate) fn is_loading_more(
        filter: PrAudience,
        state: PrSearchState,
        refine: &Refine,
        repos: &BTreeSet<RepoId>,
        cx: &App,
    ) -> bool {
        cx.try_global::<Self>().is_some_and(|list| {
            repos.iter().any(|repo| {
                list.schedule.is_loading_more(&ListKey {
                    host: repo.host().to_owned(),
                    filter,
                    state,
                    refine: refine.clone(),
                })
            })
        })
    }

    /// Read the next page of each of `hosts`' lists under `filter` and
    /// `state` that has one. A host that may not be asked now is skipped.
    /// Deferred: the caller is inside a workspace update, and an answer
    /// arriving within it would not reach that window.
    pub(crate) fn load_more(
        hosts: impl IntoIterator<Item = String>,
        filter: PrAudience,
        state: PrSearchState,
        refine: Refine,
        cx: &mut App,
    ) {
        let hosts: Vec<String> = hosts.into_iter().collect();
        cx.defer(move |cx| {
            let now = cx.background_executor().now();
            let mut due = Vec::new();
            for host in hosts {
                if !PrPoller::takes_requests(&host, now, cx) {
                    continue;
                }
                let key = ListKey {
                    host,
                    filter,
                    state,
                    refine: refine.clone(),
                };
                due.extend(cx.default_global::<Self>().schedule.begin_more(&key));
            }
            if !due.is_empty() {
                Self::notify_activity(cx);
            }
            for due in due {
                Self::search(due, cx);
            }
        });
    }

    /// A page started showing a list or changed its filter: run a round now
    /// rather than at the next tick. Deferred: the caller is inside a
    /// workspace update the round would re-enter.
    pub(crate) fn wanted(cx: &mut App) {
        cx.defer(PrPoller::tick);
    }

    /// Search `hosts` for each of `searches` under `state` again at the
    /// next round, whatever the cadence says, from the first page: the
    /// user pressed refresh, or changed GitHub in a way every tab may show.
    pub(crate) fn refresh(
        hosts: impl IntoIterator<Item = String>,
        searches: &[(PrAudience, Refine)],
        state: PrSearchState,
        cx: &mut App,
    ) {
        let schedule = &mut cx.default_global::<Self>().schedule;
        for host in hosts {
            for (filter, refine) in searches {
                schedule.refresh(ListKey {
                    host: host.clone(),
                    filter: *filter,
                    state,
                    refine: refine.clone(),
                });
            }
        }
        cx.defer(PrPoller::tick);
    }

    /// One round, run by [`PrPoller::tick`] while the feature is on: gather
    /// what every window shows, then send each search that is due.
    pub(super) fn tick(cx: &mut App) {
        let wanted = Self::wanted_now(cx);
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
        if !due.is_empty() {
            Self::notify_activity(cx);
        }
        for due in due {
            Self::search(due, cx);
        }
    }

    /// The repositories each search covers now: the union over every window
    /// showing its filter.
    fn wanted_now(cx: &mut App) -> ListWanted {
        let mut wanted: ListWanted = HashMap::new();
        WindowRegistry::for_each_workspace(cx, |ws, _, _| {
            for (filter, state, refine, repos) in ws.pr_list_demands() {
                for repo in repos {
                    let key = ListKey {
                        host: repo.host().to_owned(),
                        filter,
                        state,
                        refine: refine.clone(),
                    };
                    wanted.entry(key).or_default().insert(repo);
                }
            }
        });
        wanted
    }

    /// The feature was switched off: forget every answer and every list,
    /// and close every pull request's detail, whose buttons would run `gh`.
    pub(super) fn disable(cx: &mut App) {
        cx.default_global::<Self>().schedule.forget_answers();
        WindowRegistry::for_each_workspace(cx, |ws, window, cx| {
            ws.apply_pr_list(PrListUpdate::Disabled, cx);
            ws.close_pr_detail_while_off(window, cx);
        });
    }

    /// `gh` on `host` became unusable, or acts as another account: drop
    /// what was read there.
    pub(super) fn host_blocked(host: &str, cx: &mut App) {
        cx.default_global::<Self>().schedule.forget_host(host);
        Self::broadcast(PrListUpdate::HostBlocked { host }, cx);
    }

    /// Give a window that just started showing a filter the last answers at
    /// once. A window takes only an answer it has not seen, so this repaints
    /// nothing in a round without news.
    fn apply_cached(wanted: &ListWanted, cx: &mut App) {
        let cached = cx.default_global::<Self>().schedule.cached(wanted);
        if cached.is_empty() {
            return;
        }
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| {
            for (key, answer) in &cached {
                let update = PrListUpdate::Cached {
                    host: &key.host,
                    filter: key.filter,
                    state: key.state,
                    refine: &key.refine,
                    answer,
                };
                ws.apply_pr_list(update, cx);
            }
        });
    }

    /// Read one page per chunk, in order; the first failure fails them all,
    /// since a list missing a chunk would read as that chunk having none.
    fn search(due: Due, cx: &mut App) {
        let Due {
            key,
            epoch,
            requests,
        } = due;
        let Ok(requests) = requests else {
            let kind = requests.err().unwrap_or(FailureKind::Transient);
            cx.defer(move |cx| Self::finish(key, epoch, Err(kind), cx));
            return;
        };
        let gh = GhHost::global(cx);
        let host = key.host.clone();
        let task = cx.background_executor().spawn(async move {
            requests
                .iter()
                .map(|request| gh.search_prs_page(request))
                .collect::<Result<Vec<_>, GhError>>()
        });
        cx.spawn(async move |cx| {
            let result = task.await.map_err(|e| {
                log_failure("github.list", &host, &e);
                e.kind()
            });
            cx.update(|cx| Self::finish(key, epoch, result, cx));
        })
        .detach();
    }

    fn finish(
        key: ListKey,
        epoch: u64,
        result: Result<Vec<PrSearchPage>, FailureKind>,
        cx: &mut App,
    ) {
        let now = cx.background_executor().now();
        let enabled = super::enabled(cx);
        let outcome = cx
            .default_global::<Self>()
            .schedule
            .finish(&key, epoch, result, enabled, now);
        let (host, filter, state, refine) = (key.host.as_str(), key.filter, key.state, &key.refine);
        // A window that joined while this search ran wants repositories it
        // did not ask about, and waits on them.
        let mut uncovered = false;
        match outcome {
            ListOutcome::Answer(answer) => {
                uncovered = Self::wanted_now(cx)
                    .get(&key)
                    .is_some_and(|repos| !repos.is_subset(&answer.covered));
                PrPoller::host_answered(host, cx);
                let update = PrListUpdate::Answer {
                    host,
                    filter,
                    state,
                    refine,
                    answer: &answer,
                };
                Self::broadcast(update, cx);
            }
            ListOutcome::Failed(kind) => {
                if kind != FailureKind::Transient {
                    PrPoller::host_failed(host, kind, cx);
                }
                // Other tabs may still be answering after this host was
                // blocked; their failures must not restore its cleared state.
                if !super::is_unusable(PrPoller::availability(host, cx)) {
                    Self::broadcast(
                        PrListUpdate::Failed {
                            host,
                            filter,
                            state,
                            refine,
                        },
                        cx,
                    );
                }
            }
            // A list dropped while its search ran and shown again since
            // waits on a fresh search, sent now rather than at the next tick.
            ListOutcome::Dropped => uncovered = enabled && Self::wanted_now(cx).contains_key(&key),
        }
        Self::notify_activity(cx);
        // A refresh asked for while this search ran, or the search for what
        // this answer left out, is sent now rather than at the next tick:
        // the user is waiting on it.
        if uncovered || cx.default_global::<Self>().schedule.is_requested(&key) {
            cx.defer(PrPoller::tick);
        }
    }

    fn broadcast(update: PrListUpdate<'_>, cx: &mut App) {
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| ws.apply_pr_list(update, cx));
    }

    fn notify_activity(cx: &mut App) {
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| {
            if ws.pr_list().is_shown() {
                cx.notify();
            }
        });
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
            PrList::refresh(
                ["github.com".to_owned()],
                &[(PrAudience::Mine, Refine::default())],
                PrSearchState::Open,
                cx,
            );
        });
        cx.run_until_parked();
        assert!(gh.calls().is_empty());
    }
}
