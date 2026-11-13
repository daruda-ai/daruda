use std::collections::HashMap;

use super::*;

const HOST: &str = "github.com";
const MINUTE: Duration = Duration::from_secs(60);
const FOCUSED: Duration = Duration::from_secs(30);

fn repo(name: &str) -> RepoId {
    RepoId::new(HOST, "o", name)
}

fn demand(branches: &[&str], focused: bool) -> RepoDemand {
    RepoDemand {
        branches: branches.iter().map(|b| (*b).to_owned()).collect(),
        focused,
    }
}

fn wanted(entries: &[(&RepoId, RepoDemand)]) -> BTreeMap<RepoId, RepoDemand> {
    entries
        .iter()
        .map(|(repo, demand)| ((*repo).clone(), demand.clone()))
        .collect()
}

/// An answer covering `branches`, none of which has a PR.
fn answer(branches: &[&str]) -> RepoPrs {
    RepoPrs {
        default_branch: Some("main".into()),
        by_branch: branches
            .iter()
            .map(|b| ((*b).to_owned(), Vec::new()))
            .collect::<HashMap<_, _>>(),
    }
}

/// A schedule whose host answered `Ready` at `now`.
fn ready(now: Instant) -> Schedule {
    let mut schedule = Schedule::default();
    schedule.begin_auth_checks([HOST.to_owned()], now);
    let availability = schedule.finish_auth(HOST, Some(GhAuth::Ready), true, now);
    schedule.set_availability(HOST, availability.unwrap());
    schedule
}

/// Poll `r` for `branches` and answer it at `at`.
fn answered(schedule: &mut Schedule, r: &RepoId, branches: &[&str], at: Instant) {
    let due = schedule.begin_polls(wanted(&[(r, demand(branches, true))]), FOCUSED, at);
    assert_eq!(due.len(), 1, "the first poll is due");
    schedule.finish_poll(r, Ok(answer(branches)), true, at);
}

#[test]
fn a_host_is_asked_once_until_its_answer_goes_stale() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    assert_eq!(
        schedule.begin_auth_checks([HOST.to_owned()], now),
        vec![HOST.to_owned()]
    );
    assert!(
        schedule
            .begin_auth_checks([HOST.to_owned()], now)
            .is_empty(),
        "a check already running is not repeated"
    );
    let missing = schedule.finish_auth(HOST, Some(GhAuth::Missing), true, now);
    schedule.set_availability(HOST, missing.unwrap());
    let cases = [
        ("missing, recently answered", MINUTE, false),
        ("missing, answer gone stale", AUTH_RECHECK, true),
    ];
    for (label, after, expected) in cases {
        let mut probe = Schedule::default();
        probe.hosts.insert(
            HOST.to_owned(),
            HostState {
                availability: GhAvailability::Missing,
                auth: AuthCheck::AnsweredAt(now),
                rate_limit_failures: 0,
            },
        );
        let due = !probe
            .begin_auth_checks([HOST.to_owned()], now + after)
            .is_empty();
        assert_eq!(due, expected, "{label}");
    }
}

#[test]
fn a_ready_host_is_asked_again_only_on_recheck() {
    let now = Instant::now();
    let mut schedule = ready(now);
    assert!(
        schedule
            .begin_auth_checks([HOST.to_owned()], now + AUTH_RECHECK * 10)
            .is_empty()
    );
    schedule.recheck(HOST);
    assert_eq!(schedule.begin_auth_checks([HOST.to_owned()], now).len(), 1);
    schedule.recheck(HOST);
    assert!(
        schedule
            .begin_auth_checks([HOST.to_owned()], now)
            .is_empty(),
        "a recheck while one runs does not start another"
    );
}

#[test]
fn an_auth_answer_after_the_switch_is_off_is_dropped_and_asked_again() {
    let now = Instant::now();
    let mut schedule = Schedule::default();
    schedule.begin_auth_checks([HOST.to_owned()], now);
    assert_eq!(
        schedule.finish_auth(HOST, Some(GhAuth::Ready), false, now),
        None
    );
    assert_eq!(schedule.availability(HOST), GhAvailability::Unknown);
    assert_eq!(schedule.begin_auth_checks([HOST.to_owned()], now).len(), 1);
}

#[test]
fn a_repository_is_polled_when_its_answer_is_missing_stale_or_short() {
    let now = Instant::now();
    let r = repo("r");
    // (label, branches asked, focused, time since the answer, nudged, due)
    type Row = (
        &'static str,
        &'static [&'static str],
        bool,
        Duration,
        bool,
        bool,
    );
    let cases: [Row; 6] = [
        (
            "fresh focused answer",
            &["a"],
            true,
            MINUTE / 6,
            false,
            false,
        ),
        ("stale focused answer", &["a"], true, FOCUSED, false, true),
        (
            "unfocused uses the long cadence",
            &["a"],
            false,
            FOCUSED,
            false,
            false,
        ),
        (
            "stale unfocused answer",
            &["a"],
            false,
            UNFOCUSED_INTERVAL,
            false,
            true,
        ),
        (
            "a branch the answer did not cover",
            &["a", "b"],
            true,
            Duration::ZERO,
            false,
            true,
        ),
        ("nudged", &["a"], true, Duration::ZERO, true, true),
    ];
    for (label, branches, focused, after, nudged, expected) in cases {
        let mut schedule = ready(now);
        answered(&mut schedule, &r, &["a"], now);
        if nudged {
            schedule.nudge(r.clone());
        }
        let due = schedule.begin_polls(
            wanted(&[(&r, demand(branches, focused))]),
            FOCUSED,
            now + after,
        );
        assert_eq!(!due.is_empty(), expected, "{label}");
    }
}

#[test]
fn nothing_is_polled_on_a_host_that_cannot_answer_or_while_in_flight() {
    let now = Instant::now();
    let r = repo("r");
    let ask = || wanted(&[(&r, demand(&["a"], true))]);

    let mut unknown = Schedule::default();
    assert!(
        unknown.begin_polls(ask(), FOCUSED, now).is_empty(),
        "host never answered"
    );

    let mut schedule = ready(now);
    assert_eq!(schedule.begin_polls(ask(), FOCUSED, now).len(), 1);
    assert!(
        schedule
            .begin_polls(ask(), FOCUSED, now + UNFOCUSED_INTERVAL)
            .is_empty(),
        "a request in flight is not repeated"
    );
}

#[test]
fn transient_failures_pause_the_repository_alone_and_grow() {
    let now = Instant::now();
    let (r, other) = (repo("r"), repo("other"));
    let mut schedule = ready(now);
    schedule.begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, now);
    assert_eq!(
        schedule.finish_poll(&r, Err(FailureKind::Transient), true, now),
        PollOutcome::Paused
    );
    let both = || wanted(&[(&r, demand(&["a"], true)), (&other, demand(&["a"], true))]);
    let due = schedule.begin_polls(both(), FOCUSED, now + MINUTE / 2);
    assert_eq!(
        due,
        vec![(other.clone(), vec!["a".to_owned()])],
        "only the failing one waits"
    );

    let retry = now + MINUTE;
    assert_eq!(
        schedule
            .begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, retry)
            .len(),
        1
    );
    schedule.finish_poll(&r, Err(FailureKind::Transient), true, retry);
    let ask = || wanted(&[(&r, demand(&["a"], true))]);
    assert!(
        schedule
            .begin_polls(ask(), FOCUSED, retry + MINUTE)
            .is_empty(),
        "the second failure waits two minutes"
    );
    assert_eq!(
        schedule
            .begin_polls(ask(), FOCUSED, retry + MINUTE * 2)
            .len(),
        1
    );
}

#[test]
fn a_rate_limit_heard_by_many_repositories_grows_the_pause_once() {
    let now = Instant::now();
    let (r, other) = (repo("r"), repo("other"));
    let mut schedule = ready(now);
    schedule.begin_polls(
        wanted(&[(&r, demand(&["a"], true)), (&other, demand(&["a"], true))]),
        FOCUSED,
        now,
    );
    let first = schedule.finish_poll(&r, Err(FailureKind::RateLimited), true, now);
    let PollOutcome::Host(availability) = first else {
        panic!("a rate limit pauses the host: {first:?}");
    };
    schedule.set_availability(HOST, availability);
    assert_eq!(
        schedule.finish_poll(&other, Err(FailureKind::RateLimited), true, now),
        PollOutcome::Paused,
        "the same round's second answer does not grow the pause"
    );
    assert_eq!(schedule.hosts[HOST].rate_limit_failures, 1);
}

#[test]
fn an_answer_after_a_pause_resets_the_counts() {
    let now = Instant::now();
    let r = repo("r");
    let mut schedule = ready(now);
    schedule.begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, now);
    let PollOutcome::Host(paused) =
        schedule.finish_poll(&r, Err(FailureKind::RateLimited), true, now)
    else {
        panic!("rate limit");
    };
    schedule.set_availability(HOST, paused);
    let later = now + UNFOCUSED_INTERVAL;
    schedule.begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, later);
    let outcome = schedule.finish_poll(&r, Ok(answer(&["a"])), true, later);
    assert!(matches!(outcome, PollOutcome::Answer(_)));
    assert!(
        schedule.recovered(HOST),
        "the poller sets the host ready again"
    );
    assert_eq!(schedule.hosts[HOST].rate_limit_failures, 0);
}

#[test]
fn a_poll_answered_after_the_switch_is_off_brings_nothing_back() {
    let now = Instant::now();
    let r = repo("r");
    let mut schedule = ready(now);
    schedule.begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, now);
    assert_eq!(
        schedule.finish_poll(&r, Ok(answer(&["a"])), false, now),
        PollOutcome::Paused
    );
    assert!(
        schedule
            .cached(&wanted(&[(&r, demand(&["a"], true))]))
            .is_empty()
    );
    assert!(
        !schedule.repos[&r].in_flight,
        "the next round may ask again"
    );
}

#[test]
fn a_poll_finding_gh_gone_counts_as_an_auth_check() {
    let now = Instant::now();
    let r = repo("r");
    let mut schedule = ready(now);
    schedule.begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, now);
    assert_eq!(
        schedule.finish_poll(&r, Err(FailureKind::LoggedOut), true, now),
        PollOutcome::Host(GhAvailability::LoggedOut)
    );
    schedule.set_availability(HOST, GhAvailability::LoggedOut);
    assert!(
        schedule
            .begin_auth_checks([HOST.to_owned()], now + MINUTE)
            .is_empty(),
        "not re-asked at once"
    );
}

#[test]
fn forgetting_drops_answers_but_keeps_backoff() {
    let now = Instant::now();
    let (r, kept, elsewhere) = (
        repo("r"),
        repo("kept"),
        RepoId::new("ghe.example", "o", "x"),
    );
    let mut schedule = ready(now);
    answered(&mut schedule, &r, &["a"], now);
    answered(&mut schedule, &kept, &["a"], now);
    schedule.nudge(r.clone());
    schedule.begin_polls(wanted(&[(&r, demand(&["a"], true))]), FOCUSED, now);
    schedule.finish_poll(&r, Err(FailureKind::Transient), true, now);

    schedule.forget_unwanted(&wanted(&[(&kept, demand(&["a"], true))]));
    assert_eq!(schedule.repos[&r].answer, None, "no lane wants it");
    assert!(schedule.repos[&r].backoff.is_some(), "its backoff stays");
    assert!(schedule.repos[&kept].answer.is_some());

    schedule.repos.entry(elsewhere.clone()).or_default().answer =
        Some((Arc::new(answer(&["a"])), now));
    assert!(schedule.set_availability(HOST, GhAvailability::LoggedOut));
    assert_eq!(schedule.repos[&kept].answer, None, "its host logged out");
    assert!(
        schedule.repos[&elsewhere].answer.is_some(),
        "another host keeps its answer"
    );
    assert!(
        !schedule.set_availability(HOST, GhAvailability::LoggedOut),
        "no change"
    );
}
