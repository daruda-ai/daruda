//! When to ask `gh` about which host and repository, and what each answer
//! does to that plan. GPUI-free: [`super::PrPoller`] gathers the lanes and
//! carries out the effects; every rule about timing and backoff is here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::availability::{BackoffReason, GhAuth, GhAvailability, backoff_delay};
use daruda_github::query::RepoPrs;
use daruda_github::{FailureKind, RepoId};

/// The cadence of a repository no window is focused on.
const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long a missing or logged-out `gh` is trusted before asking again.
const AUTH_RECHECK: Duration = Duration::from_secs(5 * 60);

/// One repository's share of a round: its branches, and whether any lane
/// on it is focused.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct RepoDemand {
    pub branches: BTreeSet<String>,
    pub focused: bool,
}

/// Where `gh auth status` for a host stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthCheck {
    /// Ask at the next round.
    Due,
    Running,
    AnsweredAt(Instant),
}

#[derive(Debug)]
struct HostState {
    availability: GhAvailability,
    auth: AuthCheck,
    rate_limit_failures: u32,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            availability: GhAvailability::Unknown,
            auth: AuthCheck::Due,
            rate_limit_failures: 0,
        }
    }
}

impl HostState {
    fn auth_is_due(&self, now: Instant) -> bool {
        let answered_at = match self.auth {
            AuthCheck::Running => return false,
            AuthCheck::Due => return true,
            AuthCheck::AnsweredAt(at) => at,
        };
        match self.availability {
            GhAvailability::Unknown => true,
            GhAvailability::Missing | GhAvailability::LoggedOut => {
                now.saturating_duration_since(answered_at) >= AUTH_RECHECK
            }
            GhAvailability::Ready | GhAvailability::Backoff { .. } => false,
        }
    }

    /// A poll that finds `gh` gone or logged out counts as an auth check,
    /// so the host is not re-asked at once. A check already running stays.
    fn answered_by_poll(&mut self, now: Instant) {
        if self.auth != AuthCheck::Running {
            self.auth = AuthCheck::AnsweredAt(now);
        }
    }

    fn takes_requests(&self, now: Instant) -> bool {
        match self.availability {
            GhAvailability::Ready => true,
            GhAvailability::Backoff { until, .. } => now >= until,
            GhAvailability::Unknown | GhAvailability::Missing | GhAvailability::LoggedOut => false,
        }
    }

    /// Every repository on the host hears the same limit in one round; it
    /// grows the pause once, not per answer.
    fn rate_limited(&mut self, now: Instant) -> Option<GhAvailability> {
        if let GhAvailability::Backoff {
            until,
            reason: BackoffReason::RateLimited,
        } = self.availability
            && until > now
        {
            return None;
        }
        self.rate_limit_failures += 1;
        Some(GhAvailability::Backoff {
            until: now + backoff_delay(self.rate_limit_failures, BackoffReason::RateLimited),
            reason: BackoffReason::RateLimited,
        })
    }
}

/// A repository paused after consecutive transient failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RepoBackoff {
    failures: u32,
    until: Instant,
}

#[derive(Debug, Default)]
struct RepoState {
    /// The last answer and when it came, re-applied every round so a lane
    /// that appears after it links at once.
    answer: Option<(Arc<RepoPrs>, Instant)>,
    in_flight: bool,
    /// Ask at the next round whatever the cadence says.
    nudged: bool,
    backoff: Option<RepoBackoff>,
}

impl RepoState {
    fn is_due(&self, demand: &RepoDemand, focused_interval: Duration, now: Instant) -> bool {
        if self.in_flight || self.backoff.is_some_and(|b| now < b.until) {
            return false;
        }
        let Some((last, at)) = &self.answer else {
            return true;
        };
        // A branch the last answer did not cover is asked about at once.
        let uncovered = demand
            .branches
            .iter()
            .any(|branch| !last.by_branch.contains_key(branch));
        let interval = if demand.focused {
            focused_interval
        } else {
            UNFOCUSED_INTERVAL
        };
        self.nudged || uncovered || now.saturating_duration_since(*at) >= interval
    }

    fn failed_transiently(&mut self, now: Instant) {
        let failures = self.backoff.map_or(0, |b| b.failures) + 1;
        self.backoff = Some(RepoBackoff {
            failures,
            until: now + backoff_delay(failures, BackoffReason::Transient),
        });
    }
}

/// What a finished poll asks the poller to do.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum PollOutcome {
    /// Apply this answer to every lane on the repository.
    Answer(Arc<RepoPrs>),
    /// The repository's host changed state; see [`Schedule::set_availability`].
    Host(GhAvailability),
    /// Nothing for the lanes; the repository waits out its own backoff.
    Paused,
}

/// What a failed request means for its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum HostVerdict {
    /// The host's availability becomes this; see [`Schedule::set_availability`].
    Changed(GhAvailability),
    /// A rate-limit pause already running covers this failure.
    AlreadyPaused,
    /// The failure concerns only the request that made it.
    NotHostWide,
}

/// Every host's and repository's place in the polling plan.
#[derive(Debug, Default)]
pub(super) struct Schedule {
    hosts: HashMap<String, HostState>,
    repos: HashMap<RepoId, RepoState>,
}

impl Schedule {
    pub(super) fn availability(&self, host: &str) -> GhAvailability {
        self.hosts
            .get(host)
            .map_or(GhAvailability::Unknown, |state| state.availability)
    }

    pub(super) fn default_branch(&self, repo: &RepoId) -> Option<String> {
        let (last, _) = self.repos.get(repo)?.answer.as_ref()?;
        last.default_branch.clone()
    }

    pub(super) fn nudge(&mut self, repo: RepoId) {
        self.repos.entry(repo).or_default().nudged = true;
    }

    /// The user says they installed `gh` or logged in. A check already
    /// running answers that, so a second one is not started beside it.
    pub(super) fn recheck(&mut self, host: &str) {
        if let Some(state) = self.hosts.get_mut(host)
            && state.auth != AuthCheck::Running
        {
            state.auth = AuthCheck::Due;
        }
    }

    #[cfg(feature = "screenshot")]
    pub(super) fn seed(&mut self, host: &str, availability: GhAvailability, now: Instant) {
        self.hosts.insert(
            host.to_owned(),
            HostState {
                availability,
                auth: AuthCheck::AnsweredAt(now),
                rate_limit_failures: 0,
            },
        );
    }

    /// The feature was turned off: answers from before must not be
    /// re-applied after it is turned back on.
    pub(super) fn forget_answers(&mut self) {
        for state in self.repos.values_mut() {
            state.answer = None;
        }
    }

    /// Drop the answers of repositories no window shows any more, so one
    /// reopened later is not shown its old state before a fresh answer.
    /// Backoff stays: a failing repository must not be retried early.
    pub(super) fn forget_unwanted(&mut self, wanted: &BTreeMap<RepoId, RepoDemand>) {
        for (repo, state) in &mut self.repos {
            if !wanted.contains_key(repo) {
                state.answer = None;
            }
        }
    }

    /// The last answers of the wanted repositories that have one.
    pub(super) fn cached(
        &self,
        wanted: &BTreeMap<RepoId, RepoDemand>,
    ) -> Vec<(RepoId, Arc<RepoPrs>)> {
        wanted
            .keys()
            .filter_map(|repo| {
                let (last, _) = self.repos.get(repo)?.answer.as_ref()?;
                Some((repo.clone(), last.clone()))
            })
            .collect()
    }

    /// The hosts whose `gh auth status` should be asked now, marked running.
    pub(super) fn begin_auth_checks(
        &mut self,
        hosts: impl IntoIterator<Item = String>,
        now: Instant,
    ) -> Vec<String> {
        let mut due = Vec::new();
        for host in hosts {
            let state = self.hosts.entry(host.clone()).or_default();
            if state.auth_is_due(now) {
                state.auth = AuthCheck::Running;
                due.push(host);
            }
        }
        due
    }

    /// `gh auth status` answered (`auth` is `None` when running it failed).
    /// Returns the host's new availability, or `None` when the answer is not
    /// wanted any more (the feature was turned off while it ran); the host
    /// is asked again once it is back on.
    pub(super) fn finish_auth(
        &mut self,
        host: &str,
        auth: Option<GhAuth>,
        enabled: bool,
        now: Instant,
    ) -> Option<GhAvailability> {
        let state = self.hosts.get_mut(host)?;
        if !enabled {
            state.auth = AuthCheck::Due;
            return None;
        }
        state.auth = AuthCheck::AnsweredAt(now);
        Some(match auth {
            Some(auth) => GhAvailability::from(auth),
            None => GhAvailability::Backoff {
                until: now + backoff_delay(1, BackoffReason::Transient),
                reason: BackoffReason::Transient,
            },
        })
    }

    /// The repositories to ask now with the branches to ask about, marked
    /// in flight.
    pub(super) fn begin_polls(
        &mut self,
        wanted: BTreeMap<RepoId, RepoDemand>,
        focused_interval: Duration,
        now: Instant,
    ) -> Vec<(RepoId, Vec<String>)> {
        let mut due = Vec::new();
        for (repo, demand) in wanted {
            if !self.takes_requests(repo.host(), now) {
                continue;
            }
            let state = self.repos.entry(repo.clone()).or_default();
            if !state.is_due(&demand, focused_interval, now) {
                continue;
            }
            state.in_flight = true;
            state.nudged = false;
            due.push((repo, demand.branches.into_iter().collect()));
        }
        due
    }

    /// A poll answered. `enabled` false: the feature was turned off while it
    /// ran, so nothing comes back to the lanes.
    pub(super) fn finish_poll(
        &mut self,
        repo: &RepoId,
        result: Result<RepoPrs, FailureKind>,
        enabled: bool,
        now: Instant,
    ) -> PollOutcome {
        self.repos.entry(repo.clone()).or_default().in_flight = false;
        if !enabled {
            return PollOutcome::Paused;
        }
        let result = match result {
            Ok(prs) => {
                self.host_answered(repo.host());
                Ok(prs)
            }
            Err(kind) => Err(self.host_failed(repo.host(), kind, now)),
        };
        let state = self.repos.entry(repo.clone()).or_default();
        match result {
            Ok(prs) => {
                let prs = Arc::new(prs);
                state.answer = Some((prs.clone(), now));
                state.backoff = None;
                PollOutcome::Answer(prs)
            }
            Err(HostVerdict::Changed(availability)) => PollOutcome::Host(availability),
            Err(HostVerdict::AlreadyPaused) => PollOutcome::Paused,
            // One repository failing (gone, no access, a network blip)
            // pauses that repository alone.
            Err(HostVerdict::NotHostWide) => {
                state.failed_transiently(now);
                PollOutcome::Paused
            }
        }
    }

    /// A request about `host` succeeded: the rate-limit pause starts over.
    pub(super) fn host_answered(&mut self, host: &str) {
        if let Some(host) = self.hosts.get_mut(host) {
            host.rate_limit_failures = 0;
        }
    }

    /// What a failed request about `host` says about the whole host. Every
    /// request kind reports here, so the list search and the lane polls
    /// share one notion of a host's state.
    pub(super) fn host_failed(
        &mut self,
        host: &str,
        kind: FailureKind,
        now: Instant,
    ) -> HostVerdict {
        let state = self.hosts.get_mut(host);
        match (kind, state) {
            (FailureKind::Missing, state) => {
                if let Some(state) = state {
                    state.answered_by_poll(now);
                }
                HostVerdict::Changed(GhAvailability::Missing)
            }
            (FailureKind::LoggedOut, state) => {
                if let Some(state) = state {
                    state.answered_by_poll(now);
                }
                HostVerdict::Changed(GhAvailability::LoggedOut)
            }
            (FailureKind::RateLimited, Some(state)) => state
                .rate_limited(now)
                .map_or(HostVerdict::AlreadyPaused, HostVerdict::Changed),
            (FailureKind::RateLimited, None) => HostVerdict::AlreadyPaused,
            (FailureKind::Transient, _) => HostVerdict::NotHostWide,
        }
    }

    /// Whether requests may go to `host` now.
    pub(super) fn takes_requests(&self, host: &str, now: Instant) -> bool {
        self.hosts
            .get(host)
            .is_some_and(|state| state.takes_requests(now))
    }

    /// The one write of a host's availability. Returns whether it changed.
    /// A host where `gh` is gone or logged out can no longer keep its lanes'
    /// PRs current, so its answers are dropped rather than shown stale.
    pub(super) fn set_availability(&mut self, host: &str, availability: GhAvailability) -> bool {
        let Some(state) = self.hosts.get_mut(host) else {
            return false;
        };
        if state.availability == availability {
            return false;
        }
        state.availability = availability;
        if is_unusable(availability) {
            for (repo, state) in &mut self.repos {
                if repo.host() == host {
                    state.answer = None;
                }
            }
        }
        true
    }

    /// After a successful answer: a host paused by a backoff that has run
    /// out is usable again.
    pub(super) fn recovered(&self, host: &str) -> bool {
        matches!(self.availability(host), GhAvailability::Backoff { .. })
    }
}

/// `gh` cannot be asked about the host at all.
pub(crate) fn is_unusable(availability: GhAvailability) -> bool {
    matches!(
        availability,
        GhAvailability::Missing | GhAvailability::LoggedOut
    )
}

#[cfg(test)]
mod tests;
