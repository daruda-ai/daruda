//! When to search a host's open pull requests for a filter, and what each
//! answer does to that plan. GPUI-free: [`super::list::PrList`] gathers what
//! the windows show and carries out the effects.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::{FailureKind, PrFilter, PrSearch, RepoId};

use super::PrListAnswer;
use super::schedule::TransientBackoff;

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
    InFlight {
        repos: BTreeSet<RepoId>,
        /// The user asked to refresh after it was sent. It may have read
        /// GitHub before what they just did there, so it is asked again
        /// once it answers.
        again: bool,
    },
}

#[derive(Debug, Default)]
struct ListState {
    answer: Option<PrListAnswer>,
    ask: Ask,
    backoff: TransientBackoff,
}

impl ListState {
    fn is_due(&self, repos: &BTreeSet<RepoId>, now: Instant) -> bool {
        match self.ask {
            Ask::InFlight { .. } => return false,
            // A person asked: one request, which the pause after this
            // search's own failures exists to space out, not to refuse.
            // A paused host is still held off by `takes_requests`.
            Ask::Requested => return true,
            Ask::Idle => {}
        }
        if self.backoff.holds(now) {
            return false;
        }
        let Some(answer) = &self.answer else {
            return true;
        };
        !repos.is_subset(&answer.covered)
            || now.saturating_duration_since(answer.at) >= LIST_INTERVAL
    }
}

/// What a finished search asks [`super::list::PrList`] to do.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ListOutcome {
    /// Show this answer to the windows showing the filter.
    Answer(PrListAnswer),
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
    /// The user asked to refresh `key`. A search already running is not
    /// joined by a second one, but it was sent before they asked, so one
    /// more follows its answer.
    pub(super) fn refresh(&mut self, key: ListKey) {
        let state = self.lists.entry(key).or_default();
        match &mut state.ask {
            Ask::Idle => state.ask = Ask::Requested,
            Ask::Requested => {}
            Ask::InFlight { again, .. } => *again = true,
        }
    }

    /// Whether `key` waits for a refresh the next round sends.
    pub(super) fn is_requested(&self, key: &ListKey) -> bool {
        self.lists
            .get(key)
            .is_some_and(|state| state.ask == Ask::Requested)
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
                // Nobody is left to show what a refresh would bring.
                match &mut state.ask {
                    Ask::Requested => state.ask = Ask::Idle,
                    Ask::InFlight { again, .. } => *again = false,
                    Ask::Idle => {}
                }
            }
        }
    }

    /// The last answers of the wanted searches that have one.
    pub(super) fn cached(&self, wanted: &ListWanted) -> Vec<(ListKey, PrListAnswer)> {
        wanted
            .keys()
            .filter_map(|key| {
                let answer = self.lists.get(key)?.answer.clone()?;
                Some((key.clone(), answer))
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
            state.ask = Ask::InFlight {
                repos,
                again: false,
            };
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
        let (repos, again) = match std::mem::take(&mut state.ask) {
            Ask::InFlight { repos, again } => (repos, again),
            Ask::Idle | Ask::Requested => (BTreeSet::new(), false),
        };
        if !enabled {
            return ListOutcome::Dropped;
        }
        if again {
            state.ask = Ask::Requested;
        }
        match result {
            Ok(search) => {
                let answer = PrListAnswer {
                    search: Arc::new(search),
                    covered: Arc::new(repos),
                    at: now,
                };
                state.answer = Some(answer.clone());
                state.backoff = TransientBackoff::Clear;
                ListOutcome::Answer(answer)
            }
            Err(kind) => {
                // A host-wide failure pauses the host through its own state;
                // only a failure of this search alone pauses the search.
                if kind == FailureKind::Transient {
                    state.backoff.failed(now);
                }
                ListOutcome::Failed(kind)
            }
        }
    }
}

#[cfg(test)]
mod tests;
