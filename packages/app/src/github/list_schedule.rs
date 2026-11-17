//! When to search a host's pull requests for an audience and state, page by
//! page, and what each answer does to that plan. GPUI-free: [`super::list::PrList`] gathers what
//! the windows show and carries out the effects.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::{
    FailureKind, PrAudience, PrFilter, PrListItem, PrSearch, PrSearchContinuation,
    PrSearchCriteria, PrSearchPage, PrSearchRequest, PrSearchState, RepoId,
};

use super::PrListAnswer;
use super::refine::Refine;
use super::schedule::TransientBackoff;

/// How long a list answer stands before it is asked again.
pub(super) const LIST_INTERVAL: Duration = Duration::from_secs(60);
/// Results per page; GitHub's largest.
const PAGE_SIZE: u8 = 50;
/// Repositories one request may name; each chunk keeps its own cursor.
const REPOS_PER_REQUEST: usize = 20;

/// One search: a host's repositories under one audience and state, with
/// the words and facets a GitHub search adds. A changed refinement is
/// another search, so an answer to an older one is never shown for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ListKey {
    pub host: String,
    pub filter: PrFilter,
    pub state: PrSearchState,
    pub refine: Refine,
}

impl ListKey {
    fn criteria(&self) -> PrSearchCriteria {
        let mut criteria = PrSearchCriteria {
            audience: audience(self.filter),
            state: self.state,
            ..PrSearchCriteria::default()
        };
        self.refine.apply(&mut criteria);
        criteria
    }

    /// The first page of each chunk of `repos`.
    fn first_pages(&self, repos: &BTreeSet<RepoId>) -> Result<Vec<PrSearchRequest>, FailureKind> {
        let repos: Vec<RepoId> = repos.iter().cloned().collect();
        repos
            .chunks(REPOS_PER_REQUEST)
            .map(|chunk| PrSearchRequest::new(&self.host, chunk, self.criteria(), PAGE_SIZE))
            .collect::<Result<_, _>>()
            .map_err(|e| e.kind())
    }
}

/// The provider audience a tab lists.
pub(crate) fn audience(filter: PrFilter) -> PrAudience {
    match filter {
        PrFilter::Mine => PrAudience::Mine,
        PrFilter::ReviewRequested => PrAudience::ReviewRequested,
        PrFilter::AllOpen => PrAudience::All,
    }
}

/// The repositories each search covers this round: the union over every
/// window showing that audience and state, so one request serves all.
pub(super) type ListWanted = HashMap<ListKey, BTreeSet<RepoId>>;

/// Which pages a request reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fetch {
    /// Every chunk's first page: a fresh list that replaces the last.
    First,
    /// The next page of every chunk that has one, appended to the list.
    More,
}

/// Where one search stands between answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum Ask {
    /// Follow the cadence.
    #[default]
    Idle,
    /// The user asked to refresh: due at the next round.
    Requested,
    /// A request is running for the list of `epoch`.
    InFlight {
        repos: BTreeSet<RepoId>,
        fetch: Fetch,
        epoch: u64,
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
    /// Which list a running request belongs to. Dropping or replacing the
    /// list moves it on, so a later page of the old one is not appended.
    epoch: u64,
    /// The next page of every chunk of the shown list that has one.
    next: Vec<PrSearchRequest>,
    /// Pages beyond the first are shown: the cadence leaves the list
    /// alone, since a fresh first page cannot join the later ones.
    extended: bool,
    /// Some chunk can never be read to its end: GitHub's search cap, or a
    /// page that left matches out.
    capped: bool,
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
            || (!self.extended && now.saturating_duration_since(answer.at) >= LIST_INTERVAL)
    }

    /// Drop the list; a request still running for it answers nothing.
    fn forget(&mut self) {
        self.answer = None;
        self.next.clear();
        self.extended = false;
        self.capped = false;
        self.epoch += 1;
    }
}

/// A request to send: which pages of which search, for which list.
#[derive(Debug)]
pub(super) struct Due {
    pub key: ListKey,
    pub epoch: u64,
    /// One request per chunk, or why none could be made.
    pub requests: Result<Vec<PrSearchRequest>, FailureKind>,
}

/// What a finished search asks [`super::list::PrList`] to do.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ListOutcome {
    /// Show this answer to the windows showing the search.
    Answer(PrListAnswer),
    /// The search failed; a host-wide kind also changes the host's state.
    Failed(FailureKind),
    /// The feature was turned off, or the list was dropped or replaced,
    /// while it ran: nothing comes back.
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
    /// more follows its answer. A refresh reads the first page again.
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

    /// A queued or running search, including a refresh over retained rows.
    pub(super) fn is_refreshing(&self, key: &ListKey) -> bool {
        self.lists
            .get(key)
            .is_some_and(|state| state.ask != Ask::Idle)
    }

    /// Whether a later page of `key` is being read.
    pub(super) fn is_loading_more(&self, key: &ListKey) -> bool {
        self.lists.get(key).is_some_and(|state| {
            matches!(
                state.ask,
                Ask::InFlight {
                    fetch: Fetch::More,
                    ..
                }
            )
        })
    }

    /// The feature was turned off: answers from before must not be shown
    /// after it is turned back on.
    pub(super) fn forget_answers(&mut self) {
        for state in self.lists.values_mut() {
            state.forget();
        }
    }

    /// `gh` on `host` is gone, logged out, or acts as another account:
    /// what was read there can no longer be shown.
    pub(super) fn forget_host(&mut self, host: &str) {
        for (key, state) in &mut self.lists {
            if key.host == host {
                state.forget();
            }
        }
    }

    /// Drop the answers no window shows any more, so a page shown again
    /// later is not shown an old list before a fresh one. Backoff stays.
    pub(super) fn forget_unwanted(&mut self, wanted: &ListWanted) {
        for (key, state) in &mut self.lists {
            if !wanted.contains_key(key) {
                if state.answer.is_some() || !state.next.is_empty() {
                    state.forget();
                }
                // Nobody is left to show what a refresh would bring.
                match &mut state.ask {
                    Ask::Requested => state.ask = Ask::Idle,
                    Ask::InFlight { again, .. } => *again = false,
                    Ask::Idle => {}
                }
            }
        }
        // Each search typed is its own key: one nobody shows any more, and
        // nothing runs for, is let go rather than kept for every keystroke.
        // A bare tab's state stays, since its backoff must outlast a hidden
        // page.
        self.lists.retain(|key, state| {
            key.refine.is_empty() || wanted.contains_key(key) || state.ask != Ask::Idle
        });
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

    /// The first pages to read now, marked in flight. `takes_requests`
    /// says whether a host may be asked at all.
    pub(super) fn begin(
        &mut self,
        wanted: ListWanted,
        takes_requests: impl Fn(&str) -> bool,
        now: Instant,
    ) -> Vec<Due> {
        let mut due = Vec::new();
        for (key, repos) in wanted {
            if repos.is_empty() || !takes_requests(&key.host) {
                continue;
            }
            let state = self.lists.entry(key.clone()).or_default();
            if !state.is_due(&repos, now) {
                continue;
            }
            // A fresh first page starts a new list: a later page still
            // running for the old one must not join it.
            state.epoch += 1;
            let requests = key.first_pages(&repos);
            state.ask = Ask::InFlight {
                repos,
                fetch: Fetch::First,
                epoch: state.epoch,
                again: false,
            };
            due.push(Due {
                key,
                epoch: state.epoch,
                requests,
            });
        }
        due
    }

    /// Read the next page of `key`'s list now, if it has one and nothing
    /// else is running for it.
    pub(super) fn begin_more(&mut self, key: &ListKey) -> Option<Due> {
        let state = self.lists.get_mut(key)?;
        let repos = state.answer.as_ref()?.covered.as_ref().clone();
        if state.ask != Ask::Idle || state.next.is_empty() {
            return None;
        }
        state.ask = Ask::InFlight {
            repos,
            fetch: Fetch::More,
            epoch: state.epoch,
            again: false,
        };
        Some(Due {
            key: key.clone(),
            epoch: state.epoch,
            requests: Ok(state.next.clone()),
        })
    }

    /// A search answered with one page per request, in request order.
    /// `enabled` false: the feature was turned off while it ran.
    pub(super) fn finish(
        &mut self,
        key: &ListKey,
        epoch: u64,
        result: Result<Vec<PrSearchPage>, FailureKind>,
        enabled: bool,
        now: Instant,
    ) -> ListOutcome {
        let state = self.lists.entry(key.clone()).or_default();
        let (repos, fetch, again) = match &state.ask {
            Ask::InFlight {
                repos,
                fetch,
                epoch: running,
                again,
            } if *running == epoch => (repos.clone(), *fetch, *again),
            // Not this request's answer to give: the list it was for is gone.
            _ => return ListOutcome::Dropped,
        };
        if !enabled {
            // Nothing is shown while off; a refresh asked meanwhile lapses.
            state.ask = Ask::Idle;
            return ListOutcome::Dropped;
        }
        state.ask = if again { Ask::Requested } else { Ask::Idle };
        if state.epoch != epoch {
            // The list it was read for was dropped meanwhile.
            return ListOutcome::Dropped;
        }
        let pages = match result {
            Ok(pages) => pages,
            Err(kind) => {
                // A host-wide failure pauses the host through its own state;
                // only a failure of this search alone pauses the search.
                if kind == FailureKind::Transient {
                    state.backoff.failed(now);
                }
                return ListOutcome::Failed(kind);
            }
        };
        let (mut items, at, mut capped, matches) = match (fetch, &state.answer) {
            (Fetch::More, Some(held)) => (
                held.search.items.clone(),
                held.at,
                state.capped,
                held.matches,
            ),
            (Fetch::More, None) => return ListOutcome::Dropped,
            // A later page counts again what its first page counted.
            (Fetch::First, _) => (
                Vec::new(),
                now,
                false,
                pages.iter().map(|page| page.total_count).sum(),
            ),
        };
        state.next.clear();
        // A later page can repeat a pull request that moved while paging.
        let mut seen: BTreeSet<(RepoId, u64)> = items
            .iter()
            .map(|item| (item.repo.clone(), item.pr.number))
            .collect();
        for page in pages {
            for item in page.items {
                if seen.insert((item.repo.clone(), item.pr.number)) {
                    items.push(item);
                }
            }
            match page.continuation {
                PrSearchContinuation::More(next) => state.next.push(*next),
                PrSearchContinuation::Complete => {}
                PrSearchContinuation::SearchLimit | PrSearchContinuation::Incomplete => {
                    capped = true;
                }
            }
        }
        newest_first(&mut items);
        state.extended = fetch == Fetch::More;
        state.capped = capped;
        state.backoff = TransientBackoff::Clear;
        let more = !state.next.is_empty();
        let answer = PrListAnswer {
            search: Arc::new(PrSearch {
                items,
                truncated: capped || more,
            }),
            covered: Arc::new(repos),
            at,
            more,
            matches,
        };
        state.answer = Some(answer.clone());
        ListOutcome::Answer(answer)
    }
}

/// Most recently updated first, as each page is; GitHub's timestamps sort
/// as text.
fn newest_first(items: &mut [PrListItem]) {
    items.sort_by(|a, b| b.pr.updated_at.cmp(&a.pr.updated_at));
}

#[cfg(test)]
mod tests;
