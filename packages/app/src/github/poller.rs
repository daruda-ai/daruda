//! One poller for the whole process. Every window's lanes are gathered,
//! grouped by repository, and each repository is asked once per round, so
//! opening more windows does not multiply GitHub requests.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use daruda_github::availability::{BackoffReason, GhAuth, GhAvailability, backoff_delay};
use daruda_github::query::RepoPrs;
use daruda_github::{FailureKind, GhError, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use gpui::{App, Global};

use crate::gh_host::GhHost;
use crate::settings_store::SettingsStore;
use crate::window_registry::WindowRegistry;
use crate::workspace::github_ops::link::PrDemand;

/// How often the poller wakes to see what is due.
const TICK: Duration = Duration::from_secs(5);
/// The cadence of a repository no window is focused on.
const UNFOCUSED_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long a missing or logged-out `gh` is trusted before asking again.
const AUTH_RECHECK: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(crate) struct PrPoller {
    hosts: HashMap<String, HostState>,
    repos: HashMap<RepoId, RepoState>,
    /// A loop is running; set once by [`PrPoller::start`].
    started: bool,
    /// Lanes may hold links; cleared once when the feature is turned off.
    linked: bool,
}

impl Global for PrPoller {}

struct HostState {
    availability: GhAvailability,
    /// When `availability` was last decided by `gh auth status`.
    checked_at: Option<Instant>,
    checking: bool,
    rate_limit_failures: u32,
}

#[derive(Default)]
struct RepoState {
    /// The last answer, applied every round so a lane that appears after it
    /// links at once instead of waiting for the next request.
    last: Option<RepoPrs>,
    in_flight: bool,
    polled_at: Option<Instant>,
    /// Ask at the next tick whatever the cadence says.
    nudged: bool,
    failures: u32,
    backoff_until: Option<Instant>,
}

/// One repository's share of the round: its branches, and whether any lane
/// on it is focused.
#[derive(Default)]
struct RepoDemand {
    branches: BTreeSet<String>,
    focused: bool,
}

impl PrPoller {
    /// Start the polling loop. Idempotent; tests drive [`PrPoller::tick`].
    pub(crate) fn start(cx: &mut App) {
        if cx.default_global::<Self>().started {
            return;
        }
        cx.global_mut::<Self>().started = true;
        cx.spawn(async move |cx| {
            loop {
                cx.update(Self::tick);
                cx.background_executor().timer(TICK).await;
            }
        })
        .detach();
    }

    /// Ask about `repo` at the next tick, e.g. after its lane moved branch.
    /// Deferred: the caller may be inside a workspace update the tick would
    /// re-enter.
    pub(crate) fn nudge(repo: RepoId, cx: &mut App) {
        cx.default_global::<Self>()
            .repos
            .entry(repo)
            .or_default()
            .nudged = true;
        cx.defer(Self::tick);
    }

    /// Pretend `gh` already answered for `host`, for a capture.
    #[cfg(feature = "screenshot")]
    pub(crate) fn seed_for_shot(host: &str, availability: GhAvailability, cx: &mut App) {
        let now = cx.background_executor().now();
        cx.default_global::<Self>().hosts.insert(
            host.to_owned(),
            HostState {
                availability,
                checked_at: Some(now),
                checking: false,
                rate_limit_failures: 0,
            },
        );
    }

    /// What is known about `gh` for `host`.
    pub(crate) fn availability(host: &str, cx: &App) -> GhAvailability {
        cx.try_global::<Self>()
            .and_then(|poller| poller.hosts.get(host))
            .map_or(GhAvailability::Unknown, |state| state.availability)
    }

    /// Forget what `gh auth status` said for `host` so the next tick asks
    /// again: the user says they installed `gh` or logged in.
    pub(crate) fn recheck(host: &str, cx: &mut App) {
        if let Some(state) = cx.default_global::<Self>().hosts.get_mut(host) {
            state.checked_at = None;
        }
        cx.defer(Self::tick);
    }

    /// One round: gather every window's lanes, then ask about each repository
    /// that is due.
    pub(crate) fn tick(cx: &mut App) {
        let config = SettingsStore::global(cx).user().github.clone();
        cx.default_global::<Self>();
        if !config.enabled {
            let poller = cx.global_mut::<Self>();
            // Answers from before the switch must not be re-applied after it.
            for state in poller.repos.values_mut() {
                state.last = None;
            }
            if std::mem::take(&mut poller.linked) {
                WindowRegistry::for_each_workspace(cx, |ws, _, cx| ws.clear_pr_links(cx));
            }
            return;
        }

        let mut wanted: BTreeMap<RepoId, RepoDemand> = BTreeMap::new();
        WindowRegistry::for_each_workspace(cx, |ws, _, _| {
            for PrDemand {
                repo,
                branch,
                focused,
            } in ws.pr_demand()
            {
                let entry = wanted.entry(repo).or_default();
                entry.branches.insert(branch);
                entry.focused |= focused;
            }
        });

        Self::apply_cached(&wanted, cx);
        let now = cx.background_executor().now();
        let hosts: BTreeSet<String> = wanted.keys().map(|repo| repo.host.clone()).collect();
        for host in hosts {
            Self::check_host_if_due(host, now, cx);
        }
        for (repo, demand) in wanted {
            if Self::repo_is_due(&repo, &demand, config.interval(), now, cx) {
                Self::poll_repo(repo, demand.branches.into_iter().collect(), cx);
            }
        }
    }

    /// Re-apply each wanted repository's last answer. In-memory and
    /// change-gated, so a round with nothing new repaints nothing.
    fn apply_cached(wanted: &BTreeMap<RepoId, RepoDemand>, cx: &mut App) {
        let poller = cx.global::<Self>();
        let cached: Vec<(RepoId, RepoPrs)> = wanted
            .keys()
            .filter_map(|repo| {
                let last = poller.repos.get(repo)?.last.clone()?;
                Some((repo.clone(), last))
            })
            .collect();
        if cached.is_empty() {
            return;
        }
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| {
            for (repo, prs) in &cached {
                ws.apply_repo_prs(repo, prs, cx);
            }
        });
    }

    fn check_host_if_due(host: String, now: Instant, cx: &mut App) {
        let poller = cx.global_mut::<Self>();
        let state = poller.hosts.entry(host.clone()).or_insert(HostState {
            availability: GhAvailability::Unknown,
            checked_at: None,
            checking: false,
            rate_limit_failures: 0,
        });
        let stale = match state.availability {
            GhAvailability::Unknown => true,
            GhAvailability::Missing | GhAvailability::LoggedOut => state
                .checked_at
                .is_none_or(|at| now.saturating_duration_since(at) >= AUTH_RECHECK),
            GhAvailability::Ready | GhAvailability::Backoff { .. } => state.checked_at.is_none(),
        };
        if !stale || state.checking {
            return;
        }
        state.checking = true;
        let gh = GhHost::global(cx);
        let probe = host.clone();
        let task = cx
            .background_executor()
            .spawn(async move { gh.auth(&probe) });
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update(|cx| Self::finish_auth(&host, result, cx));
        })
        .detach();
    }

    fn finish_auth(host: &str, result: Result<GhAuth, GhError>, cx: &mut App) {
        let now = cx.background_executor().now();
        let poller = cx.global_mut::<Self>();
        let Some(state) = poller.hosts.get_mut(host) else {
            return;
        };
        state.checking = false;
        state.checked_at = Some(now);
        let availability = match result {
            Ok(auth) => GhAvailability::from(auth),
            Err(e) => {
                log_failure("github.auth", host, &e);
                GhAvailability::Backoff {
                    until: now + backoff_delay(1, BackoffReason::Transient),
                    reason: BackoffReason::Transient,
                }
            }
        };
        Self::set_host_availability(host, availability, cx);
        cx.defer(Self::tick);
    }

    fn repo_is_due(
        repo: &RepoId,
        demand: &RepoDemand,
        focused_interval: Duration,
        now: Instant,
        cx: &mut App,
    ) -> bool {
        let poller = cx.global_mut::<Self>();
        let host_ready = match poller.hosts.get(&repo.host).map(|h| h.availability) {
            Some(GhAvailability::Ready) => true,
            Some(GhAvailability::Backoff { until, .. }) => now >= until,
            _ => false,
        };
        if !host_ready {
            return false;
        }
        let state = poller.repos.entry(repo.clone()).or_default();
        if state.in_flight || state.backoff_until.is_some_and(|until| now < until) {
            return false;
        }
        let interval = if demand.focused {
            focused_interval
        } else {
            UNFOCUSED_INTERVAL
        };
        // A branch the last answer did not cover is asked about at once.
        let uncovered = state.last.as_ref().is_none_or(|last| {
            demand
                .branches
                .iter()
                .any(|branch| !last.by_branch.contains_key(branch))
        });
        state.nudged
            || uncovered
            || state
                .polled_at
                .is_none_or(|at| now.saturating_duration_since(at) >= interval)
    }

    fn poll_repo(repo: RepoId, branches: Vec<String>, cx: &mut App) {
        {
            let state = cx
                .global_mut::<Self>()
                .repos
                .entry(repo.clone())
                .or_default();
            state.in_flight = true;
            state.nudged = false;
        }
        let gh = GhHost::global(cx);
        let request = repo.clone();
        let task = cx
            .background_executor()
            .spawn(async move { gh.prs_for_branches(&request, &branches) });
        cx.spawn(async move |cx| {
            let result = task.await;
            cx.update(|cx| Self::finish_poll(repo, result, cx));
        })
        .detach();
    }

    fn finish_poll(repo: RepoId, result: Result<RepoPrs, GhError>, cx: &mut App) {
        let now = cx.background_executor().now();
        match result {
            Ok(prs) => {
                let poller = cx.global_mut::<Self>();
                let state = poller.repos.entry(repo.clone()).or_default();
                state.in_flight = false;
                state.polled_at = Some(now);
                state.failures = 0;
                state.backoff_until = None;
                state.last = Some(prs.clone());
                poller.linked = true;
                // A rate-limit pause that has run out ends with this answer.
                let paused = poller.hosts.get_mut(&repo.host).is_some_and(|host| {
                    host.rate_limit_failures = 0;
                    matches!(host.availability, GhAvailability::Backoff { .. })
                });
                if paused {
                    Self::set_host_availability(&repo.host, GhAvailability::Ready, cx);
                }
                WindowRegistry::for_each_workspace(cx, |ws, _, cx| {
                    ws.apply_repo_prs(&repo, &prs, cx);
                });
            }
            Err(e) => {
                log_failure("github.poll", &repo.qualified(), &e);
                let poller = cx.global_mut::<Self>();
                let state = poller.repos.entry(repo.clone()).or_default();
                state.in_flight = false;
                let host_now = match e.kind() {
                    FailureKind::Missing => Some(GhAvailability::Missing),
                    FailureKind::LoggedOut => Some(GhAvailability::LoggedOut),
                    FailureKind::RateLimited => {
                        let failures = poller.hosts.get_mut(&repo.host).map_or(1, |host| {
                            host.rate_limit_failures += 1;
                            host.rate_limit_failures
                        });
                        Some(GhAvailability::Backoff {
                            until: now + backoff_delay(failures, BackoffReason::RateLimited),
                            reason: BackoffReason::RateLimited,
                        })
                    }
                    // One repository failing (gone, no access, a network
                    // blip) pauses that repository alone.
                    FailureKind::Transient => {
                        state.failures += 1;
                        state.backoff_until =
                            Some(now + backoff_delay(state.failures, BackoffReason::Transient));
                        None
                    }
                };
                if let Some(availability) = host_now {
                    Self::set_host_availability(&repo.host, availability, cx);
                }
            }
        }
    }

    /// The one write of a host's availability; windows repaint when it changes.
    fn set_host_availability(host: &str, availability: GhAvailability, cx: &mut App) {
        let Some(state) = cx.global_mut::<Self>().hosts.get_mut(host) else {
            return;
        };
        if state.availability == availability {
            return;
        }
        state.availability = availability;
        WindowRegistry::for_each_workspace(cx, |_, _, cx| cx.notify());
    }
}

/// Polling failures are expected (offline, logged out) and repeat every
/// round, so they go to the log, never to a toast.
fn log_failure(site: &'static str, subject: &str, error: &GhError) {
    LogWriter::log(
        ErrorReport::new("GitHub pull request lookup failed")
            .severity(ErrorSeverity::Info)
            .from_error(error)
            .at(file!(), line!())
            .with_context("subject", subject.to_owned())
            .dedup(site)
            .build(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_round_without_lanes_asks_nothing(cx: &mut gpui::TestAppContext) {
        let gh = crate::test_support::fake_gh(cx);
        cx.update(|cx| {
            SettingsStore::init(cx);
            PrPoller::tick(cx);
        });
        cx.run_until_parked();
        assert!(gh.calls().is_empty());
    }
}
