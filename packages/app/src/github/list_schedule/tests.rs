use daruda_github::availability::{BackoffReason, backoff_delay};

use super::*;
use daruda_github::PrSearchContinuation;

const HOST: &str = "github.com";

fn repo(name: &str) -> RepoId {
    RepoId::new(HOST, "o", name)
}

fn key(filter: PrAudience) -> ListKey {
    ListKey {
        host: HOST.to_owned(),
        filter,
        state: PrSearchState::Open,
        refine: Refine::default(),
    }
}

/// Finish the request running for `key`'s current list.
fn finish(
    schedule: &mut ListSchedule,
    key: &ListKey,
    result: Result<Vec<PrSearchPage>, FailureKind>,
    enabled: bool,
    now: Instant,
) -> ListOutcome {
    let epoch = schedule.lists.get(key).map_or(0, |state| state.epoch);
    schedule.finish(key, epoch, result, enabled, now)
}

fn wanted(filter: PrAudience, repos: &[&str]) -> ListWanted {
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
fn answered(filter: PrAudience, now: Instant) -> ListSchedule {
    let mut schedule = ListSchedule::default();
    assert_eq!(sent(&mut schedule, wanted(filter, &["r"]), now), 1);
    finish(&mut schedule, &key(filter), Ok(Vec::new()), true, now);
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
            wanted(PrAudience::Mine, &["r"]),
            1,
        ),
        (
            "not due within the minute",
            |now| answered(PrAudience::Mine, now),
            later(59),
            wanted(PrAudience::Mine, &["r"]),
            0,
        ),
        (
            "due after the minute",
            |now| answered(PrAudience::Mine, now),
            later(60),
            wanted(PrAudience::Mine, &["r"]),
            1,
        ),
        (
            "manual refresh is due at once",
            |now| {
                let mut schedule = answered(PrAudience::Mine, now);
                schedule.refresh(key(PrAudience::Mine));
                schedule
            },
            later(1),
            wanted(PrAudience::Mine, &["r"]),
            1,
        ),
        (
            "a repository the answer did not cover is due at once",
            |now| answered(PrAudience::Mine, now),
            later(1),
            wanted(PrAudience::Mine, &["r", "s"]),
            1,
        ),
        (
            "fewer repositories are covered",
            |now| {
                let mut schedule = ListSchedule::default();
                schedule.begin(wanted(PrAudience::Mine, &["r", "s"]), ready, now);
                finish(
                    &mut schedule,
                    &key(PrAudience::Mine),
                    Ok(Vec::new()),
                    true,
                    now,
                );
                schedule
            },
            later(1),
            wanted(PrAudience::Mine, &["r"]),
            0,
        ),
        (
            "another filter is its own search",
            |now| answered(PrAudience::Mine, now),
            later(1),
            wanted(PrAudience::All, &["r"]),
            1,
        ),
        (
            "hidden stops asking",
            |now| answered(PrAudience::Mine, now),
            later(120),
            ListWanted::new(),
            0,
        ),
        (
            "no repositories, no request",
            |_| ListSchedule::default(),
            now,
            wanted(PrAudience::Mine, &[]),
            0,
        ),
    ];
    for (label, setup, at, wanted, expected) in cases {
        let mut schedule = setup(now);
        assert_eq!(sent(&mut schedule, wanted, at), expected, "{label}");
    }
}

#[test]
fn a_refresh_while_in_flight_follows_the_answer_once() {
    let now = Instant::now();
    let w = || wanted(PrAudience::Mine, &["r"]);
    /// (label, how the running search ends, follow-ups sent after it)
    type Row = (&'static str, Result<Vec<PrSearchPage>, FailureKind>, usize);
    let cases: Vec<Row> = vec![
        ("answered", Ok(Vec::new()), 1),
        // The pause after a failure spaces the cadence, not a person.
        ("failed", Err(FailureKind::Transient), 1),
    ];
    for (label, result, expected) in cases {
        let mut schedule = ListSchedule::default();
        assert_eq!(sent(&mut schedule, w(), now), 1, "{label}");
        assert!(schedule.is_refreshing(&key(PrAudience::Mine)), "{label}");
        schedule.refresh(key(PrAudience::Mine));
        schedule.refresh(key(PrAudience::Mine));
        assert_eq!(
            sent(&mut schedule, w(), now + Duration::from_secs(120)),
            0,
            "{label}: no second search beside the running one"
        );
        assert!(!schedule.is_requested(&key(PrAudience::Mine)), "{label}");

        // The running search was sent before the refresh: it may not have
        // seen what the user just changed.
        finish(&mut schedule, &key(PrAudience::Mine), result, true, now);
        assert!(schedule.is_requested(&key(PrAudience::Mine)), "{label}");
        assert!(schedule.is_refreshing(&key(PrAudience::Mine)), "{label}");
        assert_eq!(sent(&mut schedule, w(), now), expected, "{label}");
        finish(
            &mut schedule,
            &key(PrAudience::Mine),
            Ok(Vec::new()),
            true,
            now,
        );
        assert!(!schedule.is_refreshing(&key(PrAudience::Mine)), "{label}");
        assert_eq!(
            sent(&mut schedule, w(), now),
            0,
            "{label}: one follow-up, however often refresh was pressed"
        );
    }
}

#[test]
fn a_refresh_in_flight_is_dropped_with_its_reason() {
    let now = Instant::now();
    let w = || wanted(PrAudience::Mine, &["r"]);
    /// (label, what happens while the search runs, feature still on)
    type Row = (&'static str, fn(&mut ListSchedule), bool);
    let cases: Vec<Row> = vec![
        (
            "the page was hidden",
            |s| s.forget_unwanted(&ListWanted::new()),
            true,
        ),
        ("the feature was turned off", |_| {}, false),
    ];
    for (label, meanwhile, enabled) in cases {
        let mut schedule = ListSchedule::default();
        sent(&mut schedule, w(), now);
        schedule.refresh(key(PrAudience::Mine));
        meanwhile(&mut schedule);
        finish(
            &mut schedule,
            &key(PrAudience::Mine),
            Ok(Vec::new()),
            enabled,
            now,
        );
        assert!(!schedule.is_requested(&key(PrAudience::Mine)), "{label}");
    }
}

#[test]
fn a_host_that_takes_no_requests_is_not_asked() {
    let mut schedule = ListSchedule::default();
    let due = schedule.begin(wanted(PrAudience::Mine, &["r"]), |_| false, Instant::now());
    assert!(due.is_empty());
}

#[test]
fn a_transient_failure_backs_off_and_grows() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    let w = || wanted(PrAudience::Mine, &["r"]);
    assert_eq!(sent(&mut schedule, w(), now), 1);
    let outcome = finish(
        &mut schedule,
        &key(PrAudience::Mine),
        Err(FailureKind::Transient),
        true,
        now,
    );
    assert_eq!(outcome, ListOutcome::Failed(FailureKind::Transient));

    let first = backoff_delay(1, BackoffReason::Transient);
    assert_eq!(
        sent(&mut schedule, w(), now + first - Duration::from_secs(1)),
        0
    );
    let retry = now + first;
    assert_eq!(sent(&mut schedule, w(), retry), 1);
    finish(
        &mut schedule,
        &key(PrAudience::Mine),
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
fn a_refresh_asks_once_through_a_searchs_pause() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    let w = || wanted(PrAudience::Mine, &["r"]);
    sent(&mut schedule, w(), now);
    finish(
        &mut schedule,
        &key(PrAudience::Mine),
        Err(FailureKind::Transient),
        true,
        now,
    );
    let paused = now + Duration::from_secs(1);
    assert_eq!(sent(&mut schedule, w(), paused), 0, "paused");

    schedule.refresh(key(PrAudience::Mine));
    assert_eq!(sent(&mut schedule, w(), paused), 1, "Try again asks now");
    finish(
        &mut schedule,
        &key(PrAudience::Mine),
        Err(FailureKind::Transient),
        true,
        paused,
    );
    let second = backoff_delay(2, BackoffReason::Transient);
    assert_eq!(
        sent(&mut schedule, w(), paused + second - Duration::from_secs(1)),
        0,
        "a failed retry grows the pause the cadence keeps"
    );
    assert_eq!(sent(&mut schedule, w(), paused + second), 1);

    // A paused host is not asked, refresh or not.
    schedule.refresh(key(PrAudience::Mine));
    let due = schedule.begin(w(), |_| false, paused + second);
    assert!(due.is_empty());
}

#[test]
fn a_host_wide_failure_leaves_the_search_unpaused() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    sent(&mut schedule, wanted(PrAudience::Mine, &["r"]), now);
    let outcome = finish(
        &mut schedule,
        &key(PrAudience::Mine),
        Err(FailureKind::RateLimited),
        true,
        now,
    );
    assert_eq!(outcome, ListOutcome::Failed(FailureKind::RateLimited));
    assert_eq!(
        sent(&mut schedule, wanted(PrAudience::Mine, &["r"]), now),
        1,
        "the host's own pause decides; here the host still takes requests"
    );
}

#[test]
fn answers_are_cached_until_forgotten() {
    let now = Instant::now();
    let w = wanted(PrAudience::Mine, &["r"]);
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
        let mut schedule = answered(PrAudience::Mine, now);
        forget(&mut schedule);
        assert_eq!(schedule.cached(&w).len(), expected, "{label}");
    }
}

#[test]
fn an_answer_after_the_switch_is_off_is_dropped() {
    let now = Instant::now();
    let mut schedule = ListSchedule::default();
    sent(&mut schedule, wanted(PrAudience::Mine, &["r"]), now);
    let outcome = finish(
        &mut schedule,
        &key(PrAudience::Mine),
        Ok(Vec::new()),
        false,
        now,
    );
    assert_eq!(outcome, ListOutcome::Dropped);
    assert!(
        schedule
            .cached(&wanted(PrAudience::Mine, &["r"]))
            .is_empty()
    );
    assert_eq!(
        sent(&mut schedule, wanted(PrAudience::Mine, &["r"]), now),
        1,
        "no longer in flight"
    );
}

fn item(number: u64, updated: u32) -> PrListItem {
    PrListItem {
        repo: repo("r"),
        pr: daruda_github::PrSummary {
            number,
            url: String::new(),
            title: String::new(),
            state: daruda_github::PrState::Open,
            head_branch: format!("h{number}"),
            cross_repo: false,
            checks: daruda_github::CheckRollup::None,
            failed: Vec::new(),
            review: None,
            updated_at: format!("2026-01-01T00:00:{updated:02}Z"),
        },
        author: None,
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

fn next_request() -> PrSearchRequest {
    PrSearchRequest::new(HOST, &[repo("r")], PrSearchCriteria::default(), 50).unwrap()
}

fn page(items: Vec<PrListItem>, continuation: PrSearchContinuation) -> PrSearchPage {
    PrSearchPage {
        total_count: items.len() as u32,
        items,
        continuation,
    }
}

fn more() -> PrSearchContinuation {
    PrSearchContinuation::More(Box::new(next_request()))
}

fn numbers(answer: &PrListAnswer) -> Vec<u64> {
    answer
        .search
        .items
        .iter()
        .map(|item| item.pr.number)
        .collect()
}

#[test]
fn a_later_page_appends_without_repeats_and_ends_the_offer() {
    let now = Instant::now();
    let k = key(PrAudience::Mine);
    let mut schedule = ListSchedule::default();
    assert_eq!(
        sent(&mut schedule, wanted(PrAudience::Mine, &["r"]), now),
        1
    );
    let ListOutcome::Answer(first) = finish(
        &mut schedule,
        &k,
        Ok(vec![page(vec![item(3, 30), item(2, 20)], more())]),
        true,
        now,
    ) else {
        panic!("first page");
    };
    assert!(first.more && first.search.truncated);

    let due = schedule.begin_more(&k).expect("a next page to read");
    assert_eq!(due.requests.as_ref().unwrap(), &vec![next_request()]);
    assert!(schedule.is_loading_more(&k));
    assert!(schedule.begin_more(&k).is_none(), "one at a time");
    // A pull request updated while paging moves onto the next page too.
    let ListOutcome::Answer(longer) = schedule.finish(
        &k,
        due.epoch,
        Ok(vec![page(
            vec![item(2, 20), item(1, 10)],
            PrSearchContinuation::Complete,
        )]),
        true,
        now + Duration::from_secs(5),
    ) else {
        panic!("second page");
    };
    assert_eq!(numbers(&longer), [3, 2, 1]);
    assert!(!longer.more && !longer.search.truncated);
    assert_eq!(longer.at, first.at, "the list is as old as its first page");
    assert!(schedule.begin_more(&k).is_none(), "nothing further");

    assert_eq!(
        sent(
            &mut schedule,
            wanted(PrAudience::Mine, &["r"]),
            now + LIST_INTERVAL * 2
        ),
        0,
        "the cadence does not replace a longer list with its first page"
    );
    schedule.refresh(k.clone());
    assert_eq!(
        sent(
            &mut schedule,
            wanted(PrAudience::Mine, &["r"]),
            now + LIST_INTERVAL * 2
        ),
        1,
        "a refresh reads it from the top"
    );
    let ListOutcome::Answer(fresh) = finish(
        &mut schedule,
        &k,
        Ok(vec![page(
            vec![item(4, 40)],
            PrSearchContinuation::Complete,
        )]),
        true,
        now + LIST_INTERVAL * 2,
    ) else {
        panic!("fresh first page");
    };
    assert_eq!(numbers(&fresh), [4], "not joined to the older later pages");
}

#[test]
fn a_page_for_a_list_since_dropped_or_replaced_answers_nothing() {
    let now = Instant::now();
    let k = key(PrAudience::Mine);
    let start = |schedule: &mut ListSchedule| {
        sent(schedule, wanted(PrAudience::Mine, &["r"]), now);
        finish(
            schedule,
            &k,
            Ok(vec![page(vec![item(2, 20)], more())]),
            true,
            now,
        );
        schedule.begin_more(&k).unwrap()
    };
    /// (label, what happens while the later page is read)
    type Row = (&'static str, fn(&mut ListSchedule));
    let cases: Vec<Row> = vec![
        ("the host was blocked", |s| s.forget_host(HOST)),
        ("the feature was turned off", ListSchedule::forget_answers),
        ("no window shows it", |s| {
            s.forget_unwanted(&ListWanted::new())
        }),
    ];
    for (label, meanwhile) in cases {
        let mut schedule = ListSchedule::default();
        let due = start(&mut schedule);
        meanwhile(&mut schedule);
        let outcome = schedule.finish(
            &k,
            due.epoch,
            Ok(vec![page(
                vec![item(1, 10)],
                PrSearchContinuation::Complete,
            )]),
            true,
            now,
        );
        assert_eq!(outcome, ListOutcome::Dropped, "{label}");
        assert!(
            schedule
                .cached(&wanted(PrAudience::Mine, &["r"]))
                .is_empty(),
            "{label}"
        );
    }
}

#[test]
fn each_state_is_its_own_search() {
    let now = Instant::now();
    let mut schedule = answered(PrAudience::Mine, now);
    let merged = ListKey {
        state: PrSearchState::Merged,
        ..key(PrAudience::Mine)
    };
    let wanted = HashMap::from([(merged, BTreeSet::from([repo("r")]))]);
    assert_eq!(sent(&mut schedule, wanted, now), 1);
}

#[test]
fn more_than_one_requests_worth_of_repositories_is_read_in_chunks() {
    let names: Vec<String> = (0..21).map(|n| format!("r{n}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut schedule = ListSchedule::default();
    let due = schedule.begin(wanted(PrAudience::Mine, &names), ready, Instant::now());
    let requests = due[0].requests.as_ref().unwrap();
    let sizes: Vec<usize> = requests.iter().map(|r| r.repos().len()).collect();
    assert_eq!(sizes, [20, 1]);
}

#[test]
fn a_typed_search_nobody_shows_is_let_go_but_a_bare_tab_keeps_its_pause() {
    let now = Instant::now();
    let refined = |text: &str| ListKey {
        refine: Refine::parse(text),
        ..key(PrAudience::Mine)
    };
    let mut schedule = ListSchedule::default();
    for text in ["a", "ab", "abc"] {
        let wanted = HashMap::from([(refined(text), BTreeSet::from([repo("r")]))]);
        sent(&mut schedule, wanted.clone(), now);
        finish(&mut schedule, &refined(text), Ok(Vec::new()), true, now);
        schedule.forget_unwanted(&wanted);
    }
    sent(&mut schedule, wanted(PrAudience::Mine, &["r"]), now);
    finish(
        &mut schedule,
        &key(PrAudience::Mine),
        Err(FailureKind::Transient),
        true,
        now,
    );
    schedule.forget_unwanted(&ListWanted::new());
    let keys: Vec<&ListKey> = schedule.lists.keys().collect();
    assert_eq!(keys, [&key(PrAudience::Mine)], "only the bare tab is kept");
    assert_eq!(
        sent(
            &mut schedule,
            wanted(PrAudience::Mine, &["r"]),
            now + Duration::from_secs(1)
        ),
        0,
        "and its pause still holds when the page shows again"
    );
}
