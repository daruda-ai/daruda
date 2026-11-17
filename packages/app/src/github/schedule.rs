//! When to ask `gh` about which host and repository, and what each answer
//! does to that plan. GPUI-free: [`super::PrPoller`] gathers the lanes and
//! carries out the effects; every rule about timing and backoff is here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use daruda_github::RepoPrs;
use daruda_github::availability::{
    AuthStatus, BackoffReason, GhAccount, GhAvailability, Resume, backoff_delay,
};
use daruda_github::{FailureKind, RepoId};

/// The cadence of a repository no window is focused on.
const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long a missing or logged-out `gh` is trusted before asking again.
const AUTH_RECHECK: Duration = Duration::from_secs(5 * 60);

/// One repository's share of a round: its branches, the pull requests review
/// lanes name, and whether any lane on it is focused.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct RepoDemand {
    pub branches: BTreeSet<String>,
    pub numbers: BTreeSet<u64>,
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
    /// The account the last ready auth check named; dropped once `gh`
    /// cannot be asked about the host.
    account: Option<GhAccount>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            availability: GhAvailability::Unknown,
            auth: AuthCheck::Due,
            rate_limit_failures: 0,
            account: None,
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

    /// Whether a backoff still holds the host at `now`.
    fn is_paused(&self, now: Instant) -> bool {
        matches!(self.availability, GhAvailability::Backoff { until, .. } if now < until)
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
            ..
        } = self.availability
            && until > now
        {
            return None;
        }
        self.rate_limit_failures += 1;
        Some(GhAvailability::Backoff {
            until: now + backoff_delay(self.rate_limit_failures, BackoffReason::RateLimited),
            reason: BackoffReason::RateLimited,
            resume: Resume::Estimated,
        })
    }

    /// GitHub reported that the limit pausing the host resets at `until`.
    /// It replaces only an estimated rate-limit pause still running; a pause
    /// that ended or turned into something else meanwhile stands.
    fn reset_reported(&self, until: Instant, now: Instant) -> Option<GhAvailability> {
        match self.availability {
            GhAvailability::Backoff {
                until: estimated,
                reason: BackoffReason::RateLimited,
                resume: Resume::Estimated,
            } if estimated > now && until > now => Some(GhAvailability::Backoff {
                until,
                reason: BackoffReason::RateLimited,
                resume: Resume::Reported,
            }),
            _ => None,
        }
    }
}

/// The pause one kind of request keeps after its own consecutive transient
/// failures, growing with each. A host-wide failure pauses the host instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum TransientBackoff {
    #[default]
    Clear,
    Paused {
        failures: u32,
        until: Instant,
    },
}

impl TransientBackoff {
    /// Whether the pause still holds the request back at `now`.
    pub(super) fn holds(self, now: Instant) -> bool {
        match self {
            Self::Clear => false,
            Self::Paused { until, .. } => now < until,
        }
    }

    /// One more transient failure at `now`: pause, longer than the last.
    pub(super) fn failed(&mut self, now: Instant) {
        let failures = match *self {
            Self::Clear => 1,
            Self::Paused { failures, .. } => failures + 1,
        };
        *self = Self::Paused {
            failures,
            until: now + backoff_delay(failures, BackoffReason::Transient),
        };
    }
}

#[derive(Debug, Default)]
struct RepoState {
    /// The last answer and when it came, re-applied every round so a lane
    /// that appears after it links at once.
    answer: Option<(Arc<RepoPrs>, Instant)>,
    in_flight: bool,
    /// Ask at the next round whatever the cadence says.
    nudged: bool,
    backoff: TransientBackoff,
}

impl RepoState {
    fn is_due(&self, demand: &RepoDemand, focused_interval: Duration, now: Instant) -> bool {
        if self.in_flight || self.backoff.holds(now) {
            return false;
        }
        let Some((last, at)) = &self.answer else {
            return true;
        };
        // A branch or number the last answer did not cover is asked about
        // at once.
        let uncovered = demand
            .branches
            .iter()
            .any(|branch| !last.by_branch.contains_key(branch))
            || demand
                .numbers
                .iter()
                .any(|number| !last.by_number.contains_key(number));
        let interval = if demand.focused {
            focused_interval
        } else {
            UNFOCUSED_INTERVAL
        };
        self.nudged || uncovered || now.saturating_duration_since(*at) >= interval
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
                account: None,
            },
        );
    }

    #[cfg(feature = "screenshot")]
    pub(super) fn seed_account(&mut self, host: &str, account: GhAccount) {
        if let Some(state) = self.hosts.get_mut(host) {
            state.account = Some(account);
        }
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

    /// The account `gh` acts as on `host`, while it can be asked.
    pub(super) fn account(&self, host: &str) -> Option<&GhAccount> {
        self.hosts.get(host)?.account.as_ref()
    }

    /// `gh auth status` answered (`auth` is `None` when running it failed).
    /// Returns the host's new availability, or `None` when the answer is not
    /// wanted any more (the feature was turned off while it ran); the host
    /// is asked again once it is back on.
    pub(super) fn finish_auth(
        &mut self,
        host: &str,
        auth: Option<AuthStatus>,
        enabled: bool,
        now: Instant,
    ) -> Option<GhAvailability> {
        let state = self.hosts.get_mut(host)?;
        if !enabled {
            state.auth = AuthCheck::Due;
            return None;
        }
        state.auth = AuthCheck::AnsweredAt(now);
        // A failed check keeps the account it last named: the host is
        // paused, not logged out.
        if let Some(status) = &auth {
            state.account = status.account.clone();
        }
        Some(match auth {
            Some(status) => GhAvailability::from(status.auth),
            None => GhAvailability::Backoff {
                until: now + backoff_delay(1, BackoffReason::Transient),
                reason: BackoffReason::Transient,
                resume: Resume::Estimated,
            },
        })
    }

    /// The repositories to ask now with what to ask about, marked in flight.
    pub(super) fn begin_polls(
        &mut self,
        wanted: BTreeMap<RepoId, RepoDemand>,
        focused_interval: Duration,
        now: Instant,
    ) -> Vec<(RepoId, RepoDemand)> {
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
            due.push((repo, demand));
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
                self.host_answered(repo.host(), now);
                Ok(prs)
            }
            Err(kind) => Err(self.host_failed(repo.host(), kind, now)),
        };
        let state = self.repos.entry(repo.clone()).or_default();
        match result {
            Ok(prs) => {
                let prs = Arc::new(prs);
                state.answer = Some((prs.clone(), now));
                state.backoff = TransientBackoff::Clear;
                PollOutcome::Answer(prs)
            }
            Err(HostVerdict::Changed(availability)) => PollOutcome::Host(availability),
            Err(HostVerdict::AlreadyPaused) => PollOutcome::Paused,
            // One repository failing (gone, no access, a network blip)
            // pauses that repository alone.
            Err(HostVerdict::NotHostWide) => {
                state.backoff.failed(now);
                PollOutcome::Paused
            }
        }
    }

    /// A request about `host` succeeded at `now`: the rate-limit pause
    /// starts over. Not inside a pause still running, the rule
    /// [`Self::recovered`] follows: that answer is to a request sent before
    /// the limit was heard, and the next limit must still grow the pause.
    pub(super) fn host_answered(&mut self, host: &str, now: Instant) {
        if let Some(state) = self.hosts.get_mut(host)
            && !state.is_paused(now)
        {
            state.rate_limit_failures = 0;
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

    /// GitHub reported when the limit pausing `host` resets: the host's
    /// availability to set, or `None` when the pause it would replace is no
    /// longer an estimated rate-limit pause.
    pub(super) fn reset_reported(
        &self,
        host: &str,
        until: Instant,
        now: Instant,
    ) -> Option<GhAvailability> {
        self.hosts.get(host)?.reset_reported(until, now)
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
            state.account = None;
            for (repo, state) in &mut self.repos {
                if repo.host() == host {
                    state.answer = None;
                }
            }
        }
        true
    }

    /// After a successful answer at `now`: a host paused by a backoff that
    /// has run out is usable again. An answer inside the pause is to a
    /// request sent before it and says nothing about the limit since.
    pub(super) fn recovered(&self, host: &str, now: Instant) -> bool {
        matches!(
            self.availability(host),
            GhAvailability::Backoff { until, .. } if until <= now
        )
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
