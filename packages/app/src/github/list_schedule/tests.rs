use super::*;

const HOST: &str = "github.com";

fn repo(name: &str) -> RepoId {
    RepoId::new(HOST, "o", name)
}

fn key(filter: PrFilter) -> ListKey {
    ListKey {
        host: HOST.to_owned(),
        filter,
    }
}

fn wanted(filter: PrFilter, repos: &[&str]) -> ListWanted {
    HashMap::from([(key(filter), repos.iter().map(|name| repo(name)).collect())])
}

fn ready(_: &str) -> bool {
    true
}

/// How many searches a round starting at `now` sends.
fn sent(schedule: &mut ListSchedule, wanted: ListWanted, now: Instant) -> usize {
    schedule.begin(wanted, ready, now).len()
}

/// A schedule that asked `filter` over repository `r` and was answered at `now`.
fn answered(filter: PrFilter, now: Instant) -> ListSchedule {
    let mut schedule = ListSchedule::default();
    assert_eq!(sent(&mut schedule, wanted(filter, &["r"]), now), 1);
    schedule.finish(&key(filter), Ok(PrSearch::default()), true, now);
    schedule
}

#[test]
fn a_search_is_due_by_its_state() {
    let now = Instant::now();
    let later = |secs| now + Duration::from_secs(secs);
    /// (label, set up, round time, wanted, expected searches sent)
    type Row = (
        &'static str,
        fn(Instant) -> ListSchedule,
        Instant,
        ListWanted,
        usize,
    );
    let cases: Vec<Row> = vec![
        (
            "due at first",
            |_| ListSchedule::default(),
            now,
            wanted(PrFilter::Mine, &["r"]),
            1,
        ),
        (
            "not due within the minute",
            |now| answered(PrFilter::Mine, now),
            later(59),
            wanted(PrFilter::Mine, &["r"]),
            0,
        ),
        (
            "due after the minute",
            |now| answered(PrFilter::Mine, now),
            later(60),
            wanted(PrFilter::Mine, &["r"]),
            1,
        ),
        (
            "manual refresh is due at once",
            |now| {
                let mut schedule = answered(PrFilter::Mine, now);
                schedule.refresh(key(PrFilter::Mine));
                schedule
            },
            later(1),
            wanted(PrFilter::Mine, &["r"]),
            1,
        ),
        (
            "a repository the answer did not cover is due at once",
            |now| answered(PrFilter::Mine, now),
            later(1),
            wanted(PrFilter::Mine, &["r", "s"]),
            1,
        ),
        (
            "fewer repositories are covered",
            |now| {
                let mut schedule = ListSchedule::default();
                schedule.begin(wanted(PrFilter::Mine, &["r", "s"]), ready, now);
                schedule.finish(&key(PrFilter::Mine), Ok(PrSearch::default()), true, now);
                schedule
            },
            later(1),
            wanted(PrFilter::Mine, &["r"]),
            0,
        ),
        (
            "another filter is its own search",
            |now| answered(PrFilter::Mine, now),
            later(1),
            wanted(PrFilter::AllOpen, &["r"]),
            1,
        ),
        (
            "hidden stops asking",
            |now| answered(PrFilter::Mine, now),
            later(120),
            ListWanted::new(),
            0,
        ),
        (
            "no repositories, no request",
            |_| ListSchedule::default(),
            now,
            wanted(PrFilter::Mine, &[]),
            0,
        ),
    ];
    for (label, setup, at, wanted, expected) in cases {
        let mut schedule = setup(now);
        assert_eq!(sent(&mut schedule, wanted, at), expected, "{label}");
    }
}

#[test]
fn no_duplicate_while_in_flight_even_on_refresh() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    assert_eq!(sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), now), 1);
    schedule.refresh(key(PrFilter::Mine));
    let later = now + Duration::from_secs(120);
    assert_eq!(
        sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), later),
        0,
        "the running search answers the refresh"
    );
    schedule.finish(&key(PrFilter::Mine), Ok(PrSearch::default()), true, later);
    assert_eq!(
        sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), later),
        0,
        "and the refresh does not outlive it"
    );
}

#[test]
fn a_host_that_takes_no_requests_is_not_asked() {
    let mut schedule = ListSchedule::default();
    let due = schedule.begin(wanted(PrFilter::Mine, &["r"]), |_| false, Instant::now());
    assert!(due.is_empty());
}

#[test]
fn a_transient_failure_backs_off_and_grows() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    let w = || wanted(PrFilter::Mine, &["r"]);
    assert_eq!(sent(&mut schedule, w(), now), 1);
    let outcome = schedule.finish(&key(PrFilter::Mine), Err(FailureKind::Transient), true, now);
    assert_eq!(outcome, ListOutcome::Failed(FailureKind::Transient));

    let first = backoff_delay(1, BackoffReason::Transient);
    assert_eq!(
        sent(&mut schedule, w(), now + first - Duration::from_secs(1)),
        0
    );
    schedule.refresh(key(PrFilter::Mine));
    assert_eq!(
        sent(&mut schedule, w(), now + first - Duration::from_secs(1)),
        0,
        "a refresh does not cut a backoff short"
    );
    let retry = now + first;
    assert_eq!(sent(&mut schedule, w(), retry), 1);
    schedule.finish(
        &key(PrFilter::Mine),
        Err(FailureKind::Transient),
        true,
        retry,
    );
    let second = backoff_delay(2, BackoffReason::Transient);
    assert!(second > first);
    assert_eq!(sent(&mut schedule, w(), retry + first), 0, "the pause grew");
    assert_eq!(sent(&mut schedule, w(), retry + second), 1);
}

#[test]
fn a_host_wide_failure_leaves_the_search_unpaused() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), now);
    let outcome = schedule.finish(
        &key(PrFilter::Mine),
        Err(FailureKind::RateLimited),
        true,
        now,
    );
    assert_eq!(outcome, ListOutcome::Failed(FailureKind::RateLimited));
    assert_eq!(
        sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), now),
        1,
        "the host's own pause decides; here the host still takes requests"
    );
}

#[test]
fn answers_are_cached_until_forgotten() {
    let now = Instant::now();
    let w = wanted(PrFilter::Mine, &["r"]);
    /// (label, what forgets, cached answers left)
    type Row = (&'static str, fn(&mut ListSchedule), usize);
    let cases: Vec<Row> = vec![
        ("kept", |_| {}, 1),
        ("feature off", ListSchedule::forget_answers, 0),
        ("host unusable", |s| s.forget_host(HOST), 0),
        (
            "no window shows it",
            |s| s.forget_unwanted(&ListWanted::new()),
            0,
        ),
    ];
    for (label, forget, expected) in cases {
        let mut schedule = answered(PrFilter::Mine, now);
        forget(&mut schedule);
        assert_eq!(schedule.cached(&w).len(), expected, "{label}");
    }
}

#[test]
fn an_answer_after_the_switch_is_off_is_dropped() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), now);
    let outcome = schedule.finish(&key(PrFilter::Mine), Ok(PrSearch::default()), false, now);
    assert_eq!(outcome, ListOutcome::Dropped);
    assert!(schedule.cached(&wanted(PrFilter::Mine, &["r"])).is_empty());
    assert_eq!(
        sent(&mut schedule, wanted(PrFilter::Mine, &["r"]), now),
        1,
        "no longer in flight"
    );
}
