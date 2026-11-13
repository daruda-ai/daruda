//! One poller for the whole process. Every window's lanes are gathered,
//! grouped by repository, and each repository is asked once per round, so
//! opening more windows does not multiply GitHub requests.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use daruda_github::availability::{GhAuth, GhAvailability};
use daruda_github::query::RepoPrs;
use daruda_github::{FailureKind, GhError, RepoId};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use gpui::{App, Global};

use super::schedule::{HostVerdict, PollOutcome, RepoDemand, Schedule};
use super::{PrDemand, PrList, PrUpdate};
use crate::gh_host::GhHost;
use crate::settings_store::SettingsStore;
use crate::window_registry::WindowRegistry;

/// How often the poller wakes to see what is due.
const TICK: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct PrPoller {
    schedule: Schedule,
    /// A loop is running; set once by [`PrPoller::start`].
    started: bool,
    /// Lanes may hold links; cleared once when the feature is turned off.
    linked: bool,
    /// `[github] enabled` as the last round saw it, to catch it turning on.
    enabled: Option<bool>,
}

impl Global for PrPoller {}

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
        cx.default_global::<Self>().schedule.nudge(repo);
        cx.defer(Self::tick);
    }

    /// Pretend `gh` already answered for `host`, for a capture.
    #[cfg(feature = "screenshot")]
    pub(crate) fn seed_for_shot(host: &str, availability: GhAvailability, cx: &mut App) {
        let now = cx.background_executor().now();
        cx.default_global::<Self>()
            .schedule
            .seed(host, availability, now);
    }

    /// The default branch the last answer for `repo` named.
    pub(crate) fn default_branch(repo: &RepoId, cx: &App) -> Option<String> {
        cx.try_global::<Self>()?.schedule.default_branch(repo)
    }

    /// What is known about `gh` for `host`.
    pub(crate) fn availability(host: &str, cx: &App) -> GhAvailability {
        cx.try_global::<Self>()
            .map_or(GhAvailability::Unknown, |poller| {
                poller.schedule.availability(host)
            })
    }

    /// The settings changed: run a round now rather than at the next tick,
    /// so turning the feature off clears badges at once.
    pub(crate) fn settings_changed(cx: &mut App) {
        cx.defer(Self::tick);
    }

    /// Forget what `gh auth status` said for `host` so the next tick asks
    /// again: the user says they installed `gh` or logged in.
    pub(crate) fn recheck(host: &str, cx: &mut App) {
        cx.default_global::<Self>().schedule.recheck(host);
        cx.defer(Self::tick);
    }

    /// One round: gather every window's lanes, then ask about each repository
    /// that is due.
    pub(crate) fn tick(cx: &mut App) {
        let config = SettingsStore::global(cx).user().github.clone();
        let was = cx.default_global::<Self>().enabled.replace(config.enabled);
        if !config.enabled {
            let poller = cx.global_mut::<Self>();
            poller.schedule.forget_answers();
            if std::mem::take(&mut poller.linked) {
                Self::broadcast(PrUpdate::Disabled, cx);
            }
            if was != Some(false) {
                PrList::disable(cx);
            }
            return;
        }
        // Push targets are not read while the feature is off.
        if was == Some(false) {
            Self::broadcast(PrUpdate::Enabled, cx);
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
        let hosts: BTreeSet<String> = wanted.keys().map(|repo| repo.host().to_owned()).collect();
        let schedule = &mut cx.global_mut::<Self>().schedule;
        schedule.forget_unwanted(&wanted);
        let checks = schedule.begin_auth_checks(hosts, now);
        let polls = schedule.begin_polls(wanted, config.interval(), now);
        for host in checks {
            Self::check_host(host, cx);
        }
        for (repo, branches) in polls {
            Self::poll_repo(repo, branches, cx);
        }
        PrList::tick(cx);
    }

    /// Whether `host` may be asked anything now.
    pub(super) fn takes_requests(host: &str, now: std::time::Instant, cx: &App) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|poller| poller.schedule.takes_requests(host, now))
    }

    /// Another request about `host` succeeded, with the same effect on the
    /// host as a poll answer.
    pub(super) fn host_answered(host: &str, cx: &mut App) {
        let schedule = &mut cx.default_global::<Self>().schedule;
        schedule.host_answered(host);
        if schedule.recovered(host) {
            Self::set_host_availability(host, GhAvailability::Ready, cx);
        }
    }

    /// Another request about `host` failed host-wide, with the same effect
    /// on the host as a failed poll.
    pub(super) fn host_failed(host: &str, kind: FailureKind, cx: &mut App) {
        let now = cx.background_executor().now();
        let verdict = cx
            .default_global::<Self>()
            .schedule
            .host_failed(host, kind, now);
        if let HostVerdict::Changed(availability) = verdict {
            Self::set_host_availability(host, availability, cx);
        }
    }

    /// Re-apply each wanted repository's last answer. In-memory and
    /// change-gated, so a round with nothing new repaints nothing.
    fn apply_cached(wanted: &BTreeMap<RepoId, RepoDemand>, cx: &mut App) {
        let cached = cx.global::<Self>().schedule.cached(wanted);
        if cached.is_empty() {
            return;
        }
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| {
            for (repo, prs) in &cached {
                ws.apply_pr_update(PrUpdate::Answer { repo, prs }, cx);
            }
        });
    }

    fn check_host(host: String, cx: &mut App) {
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
        let enabled = super::enabled(cx);
        let auth = match result {
            Ok(auth) => Some(auth),
            Err(e) => {
                // An answer for a switched-off feature is dropped unlogged.
                if enabled {
                    log_failure("github.auth", host, &e);
                }
                None
            }
        };
        let answered = cx
            .global_mut::<Self>()
            .schedule
            .finish_auth(host, auth, enabled, now);
        if let Some(availability) = answered {
            Self::set_host_availability(host, availability, cx);
            cx.defer(Self::tick);
        }
    }

    fn poll_repo(repo: RepoId, branches: Vec<String>, cx: &mut App) {
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
        let enabled = super::enabled(cx);
        let result = result.map_err(|e| {
            if enabled {
                log_failure("github.poll", &repo.qualified(), &e);
            }
            e.kind()
        });
        let poller = cx.global_mut::<Self>();
        match poller.schedule.finish_poll(&repo, result, enabled, now) {
            PollOutcome::Answer(prs) => {
                poller.linked = true;
                // A pause that has run out ends with this answer.
                if poller.schedule.recovered(repo.host()) {
                    Self::set_host_availability(repo.host(), GhAvailability::Ready, cx);
                }
                Self::broadcast(
                    PrUpdate::Answer {
                        repo: &repo,
                        prs: &prs,
                    },
                    cx,
                );
            }
            PollOutcome::Host(availability) => {
                Self::set_host_availability(repo.host(), availability, cx);
            }
            PollOutcome::Paused => {}
        }
    }

    /// Record a host's availability and tell every window when it changed.
    fn set_host_availability(host: &str, availability: GhAvailability, cx: &mut App) {
        let changed = cx
            .global_mut::<Self>()
            .schedule
            .set_availability(host, availability);
        if changed {
            Self::broadcast(PrUpdate::Host { host, availability }, cx);
            if super::is_unusable(availability) {
                PrList::host_blocked(host, cx);
            }
        }
    }

    fn broadcast(update: PrUpdate<'_>, cx: &mut App) {
        WindowRegistry::for_each_workspace(cx, |ws, _, cx| ws.apply_pr_update(update, cx));
    }
}

/// Polling failures are expected (offline, logged out) and repeat every
/// round, so they go to the log, never to a toast.
pub(super) fn log_failure(site: &'static str, subject: &str, error: &GhError) {
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
