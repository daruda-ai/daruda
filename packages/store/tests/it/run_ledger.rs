//! The orchestrator's run delivery ledger replays to what it committed, caps
//! unreported ends, and never holds a report of an end it never recorded.

use std::time::Duration;

use daruda_runlog::testing::{assert_machine_laws, enumerate_crashes, enumerate_crashes_with};
use daruda_runlog::{CommitError, Machine as _};
use daruda_store::run_ledger::{
    DIRECTORY_HOLD, LedgerEnd, LedgerEndedAs, LedgerKey, LedgerPointers, LedgerRejected,
    ReleaseReason, RunLedgerEvent, RunLedgerMachine, RunLedgerState, RunLedgerStore, RunLocation,
    UNREPORTED_MAX,
};
use daruda_store::tasks::ExecutionRef;

fn flow(id: &str) -> LedgerKey {
    LedgerKey::Flow {
        run_id: id.to_owned(),
    }
}

fn task(id: &str) -> LedgerKey {
    LedgerKey::Task(ExecutionRef {
        task_id: id.to_owned(),
        execution_id: "e1".into(),
    })
}

fn end(what: &str) -> LedgerEnd {
    LedgerEnd {
        what: what.to_owned(),
        why: Some("because".into()),
        ended_as: LedgerEndedAs::Completed,
        text: Some("answer".into()),
        pointers: LedgerPointers {
            run_dir: Some("/run".into()),
            ..LedgerPointers::default()
        },
    }
}

fn owned(key: LedgerKey, solicited: bool) -> RunLedgerEvent {
    RunLedgerEvent::Owned {
        key,
        solicited,
        location: None,
    }
}

fn ended(key: LedgerKey, episode: u64) -> RunLedgerEvent {
    RunLedgerEvent::Ended {
        key,
        solicited: false,
        episode,
        end: end("work"),
        evicts: Vec::new(),
        ended_at_ms: None,
    }
}

/// An end the proposer dated `at_ms`.
fn ended_at(key: LedgerKey, episode: u64, at_ms: u64) -> RunLedgerEvent {
    let RunLedgerEvent::Ended {
        key,
        solicited,
        episode,
        end,
        evicts,
        ..
    } = ended(key, episode)
    else {
        unreachable!("`ended` builds an end");
    };
    RunLedgerEvent::Ended {
        key,
        solicited,
        episode,
        end,
        evicts,
        ended_at_ms: Some(at_ms),
    }
}

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const HOLD_MS: u64 = DIRECTORY_HOLD.as_millis() as u64;

fn reported(key: LedgerKey, episode: u64) -> RunLedgerEvent {
    RunLedgerEvent::Reported { key, episode }
}

fn released(key: LedgerKey, reason: ReleaseReason) -> RunLedgerEvent {
    RunLedgerEvent::Released { key, reason }
}

fn replay(events: &[RunLedgerEvent]) -> RunLedgerState {
    let mut state = RunLedgerState::default();
    for event in events {
        let mut event = event.clone();
        RunLedgerMachine::check(&state, &event).unwrap_or_else(|e| panic!("{event:?}: {e}"));
        RunLedgerMachine::assign(&state, &mut event);
        RunLedgerMachine::apply(&mut state, &event);
    }
    state
}

/// Every reopened ledger must satisfy this, whatever write a crash cut short.
fn assert_no_report_without_end(state: &RunLedgerState) {
    for entry in state.entries() {
        if let Some(reported) = entry.reported {
            let ended = entry.ended.as_ref().map(|ended| ended.episode);
            assert!(
                ended.is_some_and(|ended| ended >= reported),
                "{:?} reports episode {reported} but ended {ended:?}",
                entry.key
            );
        }
    }
}

#[test]
fn owning_ending_and_reporting_one_run_walks_its_entry_to_release() {
    let key = task("t");
    let state = replay(&[owned(key.clone(), true)]);
    let entry = state.get(&key).unwrap();
    assert!(entry.solicited);
    assert!(entry.ended.is_none());
    assert!(entry.unreported().is_none());

    let state = replay(&[owned(key.clone(), true), ended(key.clone(), 0)]);
    let entry = state.get(&key).unwrap();
    assert!(entry.solicited, "an end keeps the owner's solicitation");
    assert_eq!(entry.unreported().unwrap().end, end("work"));
    assert_eq!(state.next_episode(), 1);

    let events = [
        owned(key.clone(), true),
        ended(key.clone(), 0),
        reported(key.clone(), 0),
    ];
    let state = replay(&events);
    assert!(state.get(&key).unwrap().unreported().is_none());

    let mut all = events.to_vec();
    all.push(released(key.clone(), ReleaseReason::Reported));
    assert!(replay(&all).get(&key).is_none());
}

#[test]
fn an_older_episode_report_leaves_a_newer_end_unreported() {
    let key = flow("f");
    let state = replay(&[
        owned(key.clone(), false),
        ended(key.clone(), 0),
        ended(key.clone(), 1),
        reported(key.clone(), 0),
    ]);
    let entry = state.get(&key).unwrap();
    assert_eq!(entry.unreported().unwrap().episode, 1);
}

#[test]
fn a_report_of_an_end_never_recorded_is_rejected() {
    let key = task("t");
    let state = replay(&[owned(key.clone(), false)]);
    assert!(matches!(
        RunLedgerMachine::check(&state, &reported(key.clone(), 0)),
        Err(LedgerRejected::NotEnded { .. })
    ));
    let state = replay(&[owned(key.clone(), false), ended(key.clone(), 0)]);
    assert!(RunLedgerMachine::check(&state, &reported(key.clone(), 1)).is_err());
}

#[test]
fn reports_and_releases_of_forgotten_keys_change_nothing() {
    let state = replay(&[owned(task("kept"), false)]);
    for event in [
        reported(task("gone"), 4),
        released(task("gone"), ReleaseReason::Expired),
    ] {
        RunLedgerMachine::check(&state, &event).unwrap();
        let mut after = state.clone();
        RunLedgerMachine::apply(&mut after, &event);
        assert_eq!(after, state);
    }
}

#[test]
fn an_end_older_than_the_next_episode_changes_nothing() {
    let state = replay(&[
        ended(task("a"), 0),
        ended(task("b"), 5),
        ended(task("a"), 6),
    ]);
    assert_eq!(state.next_episode(), 7);
    for stale in [ended(task("c"), 3), ended(task("a"), 0)] {
        let after = replay(&[
            ended(task("a"), 0),
            ended(task("b"), 5),
            ended(task("a"), 6),
            stale,
        ]);
        assert_eq!(after, state);
    }
}

#[test]
fn re_recording_the_current_end_replaces_its_text_only() {
    let key = task("t");
    let mut enriched = end("work");
    enriched.text = Some("the full report".into());
    let state = replay(&[
        owned(key.clone(), true),
        ended(key.clone(), 0),
        RunLedgerEvent::Ended {
            key: key.clone(),
            solicited: true,
            episode: 0,
            end: enriched.clone(),
            evicts: Vec::new(),
            ended_at_ms: None,
        },
    ]);
    let entry = state.get(&key).unwrap();
    assert_eq!(entry.unreported().unwrap().end, enriched);
    assert_eq!(state.next_episode(), 1);
}

#[test]
fn an_end_of_a_released_episode_is_not_resurrected() {
    let key = task("t");
    let state = replay(&[
        ended(key.clone(), 0),
        reported(key.clone(), 0),
        released(key.clone(), ReleaseReason::Reported),
        ended(key.clone(), 0),
    ]);
    assert!(state.get(&key).is_none());
}

#[test]
fn unreported_ends_past_the_cap_evict_the_oldest_and_record_which() {
    let mut events = Vec::new();
    for n in 0..=UNREPORTED_MAX as u64 {
        events.push(owned(task(&n.to_string()), false));
        events.push(ended(task(&n.to_string()), n));
    }
    let state = replay(&events);
    let unreported = state
        .entries()
        .iter()
        .filter(|entry| entry.unreported().is_some())
        .count();
    assert_eq!(unreported, UNREPORTED_MAX);
    assert!(state.get(&task("0")).is_none(), "the oldest end is evicted");
    assert!(state.get(&task("1")).is_some());

    let mut log = daruda_runlog::RunLog::<RunLedgerMachine, _>::open(
        daruda_runlog::testing::FaultBackend::new(),
        "test",
    )
    .unwrap();
    let mut last = None;
    for event in events {
        last = Some(log.commit(event).unwrap());
    }
    let RunLedgerEvent::Ended { evicts, .. } = last.unwrap().event else {
        panic!("the last event is an end");
    };
    assert_eq!(evicts, vec![task("0")], "the eviction is in the record");
}

#[test]
fn reported_and_owned_only_entries_never_count_against_the_cap() {
    let mut events = vec![owned(flow("running"), true)];
    for n in 0..=UNREPORTED_MAX as u64 {
        let key = task(&n.to_string());
        events.push(ended(key.clone(), n));
        events.push(reported(key, n));
    }
    let state = replay(&events);
    assert!(state.get(&flow("running")).is_some());
    assert!(state.get(&task("0")).is_some());
}

#[test]
fn events_and_states_round_trip_and_obey_the_machine_laws() {
    let key = flow("f");
    let located = RunLedgerEvent::Owned {
        key: key.clone(),
        solicited: true,
        location: Some(RunLocation {
            run_dir: "/tree/.daruda/flow-runs/f".into(),
            lock_dir: Some("/locks".into()),
        }),
    };
    let events = vec![
        located,
        owned(task("t"), false),
        ended(key.clone(), 0),
        ended(task("t"), 1),
        reported(key.clone(), 0),
        reported(task("t"), 0),
        released(key.clone(), ReleaseReason::Reported),
        released(task("t"), ReleaseReason::Expired),
    ];
    let states = vec![
        RunLedgerState::default(),
        replay(&events[..3]),
        replay(&events[..5]),
    ];
    for state in &states {
        let bytes = serde_json::to_vec(state).unwrap();
        assert_eq!(
            &serde_json::from_slice::<RunLedgerState>(&bytes).unwrap(),
            state
        );
    }
    for event in &events {
        let bytes = serde_json::to_vec(event).unwrap();
        assert_eq!(
            &serde_json::from_slice::<RunLedgerEvent>(&bytes).unwrap(),
            event
        );
    }
    assert_machine_laws::<RunLedgerMachine>(&states, &events);
    assert!(enumerate_crashes::<RunLedgerMachine>(events).cases > 0);
}

#[test]
fn a_crash_at_any_write_never_leaves_a_report_without_its_end() {
    let key = task("t");
    let scenario = vec![
        owned(key.clone(), true),
        ended(key.clone(), 0),
        reported(key.clone(), 0),
        released(key.clone(), ReleaseReason::Reported),
        owned(flow("f"), false),
        ended(flow("f"), 1),
        reported(flow("f"), 1),
    ];
    let summary = enumerate_crashes_with::<RunLedgerMachine>(scenario, |case| {
        assert_no_report_without_end(case.reopened.state());
    });
    assert!(summary.cases > 0);
}

#[test]
fn a_store_replays_what_it_recorded_after_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let (store, state) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    assert_eq!(state, RunLedgerState::default());
    store.record(owned(task("t"), true));
    store.record(ended(task("t"), 0));
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(store.take_failure().is_none());
    drop(store);

    let (_store, state) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    let entry = state.get(&task("t")).unwrap();
    assert!(entry.solicited);
    assert_eq!(entry.unreported().unwrap().episode, 0);
}

#[test]
fn a_refused_record_is_reported_once_by_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    store.record(owned(task("t"), false));
    store.record(reported(task("t"), 0));
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        store.take_failure(),
        Some(CommitError::Rejected(LedgerRejected::NotEnded { .. }))
    ));
    assert!(store.take_failure().is_none());
}

#[test]
fn an_unreadable_ledger_is_refused_and_left_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    store.record(owned(task("t"), false));
    store.flush(Duration::from_secs(5)).unwrap();
    drop(store);
    let log = daruda_runlog::FileBackend::new(dir.path()).log_path(0);
    let mut bytes = std::fs::read(&log).unwrap();
    bytes.splice(0..0, b"not json\n".iter().copied());
    std::fs::write(&log, &bytes).unwrap();
    assert!(RunLedgerStore::in_directory(dir.path(), "test").is_err());
    assert_eq!(std::fs::read(&log).unwrap(), bytes);
}

fn held(state: &RunLedgerState, now_ms: u64) -> Vec<String> {
    let mut ids: Vec<_> = state.held_run_dirs(now_ms).into_iter().collect();
    ids.sort();
    ids
}

#[test]
fn an_unreported_flow_end_holds_its_directory_for_the_hold_then_keeps_its_summary() {
    let at = 100 * DAY_MS;
    let state = replay(&[owned(flow("f"), true), ended_at(flow("f"), 0, at)]);
    assert_eq!(held(&state, at), ["f"]);
    assert_eq!(held(&state, at + HOLD_MS - 1), ["f"]);
    assert!(held(&state, at + HOLD_MS).is_empty(), "the hold ended");
    assert_eq!(HOLD_MS, 7 * DAY_MS);
    // Only the directory's hold ends: the end is still there to deliver.
    let entry = state.get(&flow("f")).unwrap();
    assert_eq!(entry.unreported().unwrap().end, end("work"));
}

#[test]
fn only_flows_the_ledger_still_owes_hold_their_directories() {
    let at = 100 * DAY_MS;
    let state = replay(&[
        owned(flow("running"), false),
        ended(flow("undated"), 0),
        ended_at(flow("reported"), 1, at),
        reported(flow("reported"), 1),
        ended_at(flow("released"), 2, at),
        reported(flow("released"), 2),
        released(flow("released"), ReleaseReason::Reported),
        ended_at(task("t"), 3, at),
        ended_at(flow("resumed"), 4, at),
        reported(flow("resumed"), 4),
        ended_at(flow("resumed"), 5, at),
    ]);
    // An owned run with no end yet, and an end older than dated ends, have
    // no hold deadline; a reported or released end and a Task hold nothing.
    assert_eq!(
        held(&state, at + 30 * DAY_MS),
        ["running", "undated"],
        "{state:?}"
    );
    assert_eq!(held(&state, at), ["resumed", "running", "undated"]);
}

#[test]
fn re_recording_an_end_keeps_the_time_its_hold_counts_from() {
    let at = 100 * DAY_MS;
    let state = replay(&[
        ended_at(flow("f"), 0, at),
        ended_at(flow("f"), 0, at + 5 * DAY_MS),
    ]);
    assert!(held(&state, at + HOLD_MS).is_empty());

    let state = replay(&[ended(flow("f"), 0), ended_at(flow("f"), 0, at)]);
    assert!(
        held(&state, at + HOLD_MS).is_empty(),
        "an undated end takes the first time it is given"
    );
}

#[test]
fn directory_holds_replay_the_same_from_the_log() {
    let at = 100 * DAY_MS;
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    let events = [
        owned(flow("a"), true),
        ended_at(flow("a"), 0, at),
        owned(flow("b"), true),
        ended_at(flow("b"), 1, at + 3 * DAY_MS),
        owned(flow("c"), true),
    ];
    for event in &events {
        store.record(event.clone());
    }
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(store.take_failure().is_none());
    let now = at + HOLD_MS;
    let live = store.held_run_dirs(now);
    drop(store);

    let (reopened, state) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    assert_eq!(state, replay(&events));
    assert_eq!(held(&state, now), ["b", "c"]);
    assert_eq!(
        live,
        state.held_run_dirs(now),
        "the store's view is the log's"
    );
    assert_eq!(reopened.held_run_dirs(now), live);
}

#[test]
fn a_store_stops_holding_a_directory_once_its_end_is_released() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    store.record(owned(flow("f"), true));
    store.record(ended_at(flow("f"), 0, 0));
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(store.held_run_dirs(0).contains("f"));
    store.record(reported(flow("f"), 0));
    store.record(released(flow("f"), ReleaseReason::Reported));
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(store.held_run_dirs(0).is_empty());
}
