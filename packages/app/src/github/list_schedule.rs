//! When to search a host's open pull requests for a filter, and what each
//! answer does to that plan. GPUI-free: [`super::list::PrList`] gathers what
//! the windows show and carries out the effects.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::availability::{BackoffReason, backoff_delay};
use daruda_github::{FailureKind, PrFilter, PrSearch, RepoId};

/// How long a list answer stands before it is asked again.
pub(super) const LIST_INTERVAL: Duration = Duration::from_secs(60);

/// One search: a host's repositories under one filter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ListKey {
    pub host: String,
    pub filter: PrFilter,
}

/// The repositories each search covers this round: the union over every
/// window showing that filter, so one request serves all of them.
pub(super) type ListWanted = HashMap<ListKey, BTreeSet<RepoId>>;

/// Where one search stands between answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Ask {
    /// Follow the cadence.
    #[default]
    Idle,
    /// The user asked to refresh: due at the next round.
    Requested,
    /// A request for these repositories is running.
    InFlight(BTreeSet<RepoId>),
}

#[derive(Debug)]
struct ListAnswer {
    search: Arc<PrSearch>,
    /// What the request asked about; a repository outside it is not covered.
    repos: BTreeSet<RepoId>,
    at: Instant,
}

/// A search paused after consecutive transient failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListBackoff {
    failures: u32,
    until: Instant,
}

#[derive(Debug, Default)]
struct ListState {
    answer: Option<ListAnswer>,
    ask: Ask,
    backoff: Option<ListBackoff>,
}

impl ListState {
    fn is_due(&self, repos: &BTreeSet<RepoId>, now: Instant) -> bool {
        if matches!(self.ask, Ask::InFlight(_)) || self.backoff.is_some_and(|b| now < b.until) {
            return false;
        }
        let Some(answer) = &self.answer else {
            return true;
        };
        self.ask == Ask::Requested
            || !repos.is_subset(&answer.repos)
            || now.saturating_duration_since(answer.at) >= LIST_INTERVAL
    }
}

/// What a finished search asks [`super::list::PrList`] to do.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ListOutcome {
    /// Show this answer to the windows showing the filter.
    Answer(Arc<PrSearch>),
    /// The search failed; a host-wide kind also changes the host's state.
    Failed(FailureKind),
    /// The feature was turned off while it ran: nothing comes back.
    Dropped,
}

/// Every search's place in the list polling plan.
#[derive(Debug, Default)]
pub(super) struct ListSchedule {
    lists: HashMap<ListKey, ListState>,
}

impl ListSchedule {
    /// The user asked to refresh `key`. A search already running answers
    /// that, so a second one is not started beside it.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the pull requests page calls it")
    )]
    pub(super) fn refresh(&mut self, key: ListKey) {
        let state = self.lists.entry(key).or_default();
        if state.ask == Ask::Idle {
            state.ask = Ask::Requested;
        }
    }

    /// The feature was turned off: answers from before must not be shown
    /// after it is turned back on.
    pub(super) fn forget_answers(&mut self) {
        for state in self.lists.values_mut() {
            state.answer = None;
        }
    }

    /// `gh` on `host` is gone or logged out: what was read there can no
    /// longer be kept current.
    pub(super) fn forget_host(&mut self, host: &str) {
        for (key, state) in &mut self.lists {
            if key.host == host {
                state.answer = None;
            }
        }
    }

    /// Drop the answers no window shows any more, so a page shown again
    /// later is not shown an old list before a fresh one. Backoff stays.
    pub(super) fn forget_unwanted(&mut self, wanted: &ListWanted) {
        for (key, state) in &mut self.lists {
            if !wanted.contains_key(key) {
                state.answer = None;
                if state.ask == Ask::Requested {
                    state.ask = Ask::Idle;
                }
            }
        }
    }

    /// The last answers of the wanted searches that have one.
    pub(super) fn cached(&self, wanted: &ListWanted) -> Vec<(ListKey, Arc<PrSearch>)> {
        wanted
            .keys()
            .filter_map(|key| {
                let answer = self.lists.get(key)?.answer.as_ref()?;
                Some((key.clone(), answer.search.clone()))
            })
            .collect()
    }

    /// The searches to send now, marked in flight. `takes_requests` says
    /// whether a host may be asked at all.
    pub(super) fn begin(
        &mut self,
        wanted: ListWanted,
        takes_requests: impl Fn(&str) -> bool,
        now: Instant,
    ) -> Vec<(ListKey, Vec<RepoId>)> {
        let mut due = Vec::new();
        for (key, repos) in wanted {
            if repos.is_empty() || !takes_requests(&key.host) {
                continue;
            }
            let state = self.lists.entry(key.clone()).or_default();
            if !state.is_due(&repos, now) {
                continue;
            }
            let asked = repos.iter().cloned().collect();
            state.ask = Ask::InFlight(repos);
            due.push((key, asked));
        }
        due
    }

    /// A search answered. `enabled` false: the feature was turned off while
    /// it ran.
    pub(super) fn finish(
        &mut self,
        key: &ListKey,
        result: Result<PrSearch, FailureKind>,
        enabled: bool,
        now: Instant,
    ) -> ListOutcome {
        let state = self.lists.entry(key.clone()).or_default();
        let repos = match std::mem::take(&mut state.ask) {
            Ask::InFlight(repos) => repos,
            Ask::Idle | Ask::Requested => BTreeSet::new(),
        };
        if !enabled {
            return ListOutcome::Dropped;
        }
        match result {
            Ok(search) => {
                let search = Arc::new(search);
                state.answer = Some(ListAnswer {
                    search: search.clone(),
                    repos,
                    at: now,
                });
                state.backoff = None;
                ListOutcome::Answer(search)
            }
            Err(kind) => {
                // A host-wide failure pauses the host through its own state;
                // only a failure of this search alone pauses the search.
                if kind == FailureKind::Transient {
                    let failures = state.backoff.map_or(0, |b| b.failures) + 1;
                    state.backoff = Some(ListBackoff {
                        failures,
                        until: now + backoff_delay(failures, BackoffReason::Transient),
                    });
                }
                ListOutcome::Failed(kind)
            }
        }
    }
}

#[cfg(test)]
mod tests;
