//! The orchestrator's run delivery ledger replays to what it committed, caps
//! unreported ends, and never holds a report of an end it never recorded.

use std::time::Duration;

use daruda_runlog::Machine as _;
use daruda_runlog::testing::{assert_machine_laws, enumerate_crashes, enumerate_crashes_with};
use daruda_store::run_ledger::{
    DIRECTORY_HOLD, EntryState, LedgerEnd, LedgerEndedAs, LedgerKey, LedgerPointers,
    LedgerRejected, ReleaseReason, RunLedgerEvent, RunLedgerMachine, RunLedgerState,
    RunLedgerStore, RunLocation, UNREPORTED_MAX,
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
        at_ms: None,
    }
}

fn reopened(key: LedgerKey) -> RunLedgerEvent {
    RunLedgerEvent::Reopened {
        key,
        solicited: true,
        location: None,
        at_ms: None,
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

/// Every reopened ledger must satisfy this, whatever write a crash cut short:
/// owed ends are oldest first and older than their run's current one, and
/// every recorded end was numbered before the next episode.
fn assert_ends_in_order(state: &RunLedgerState) {
    for entry in state.entries() {
        let key = &entry.key;
        let episodes: Vec<_> = entry.owed().iter().map(|end| end.episode).collect();
        assert!(
            episodes.windows(2).all(|pair| pair[0] < pair[1]),
            "{key:?} owes ends out of order: {episodes:?}"
        );
        if let EntryState::Unreported { end, owed } | EntryState::Reported { end, owed } =
            &entry.state
        {
            assert!(
                owed.iter().all(|owed| owed.episode < end.episode),
                "{key:?} owes a newer end"
            );
        }
        for end in entry.latest().into_iter().chain(entry.owed()) {
            assert!(end.episode < state.next_episode(), "{key:?}");
        }
    }
}

fn owed_episodes(state: &RunLedgerState, key: &LedgerKey) -> Vec<u64> {
    let entry = state.get(key).unwrap();
    entry.owed().iter().map(|end| end.episode).collect()
}

#[test]
fn owning_ending_and_reporting_one_run_walks_its_entry_to_release() {
    let key = task("t");
    let state = replay(&[owned(key.clone(), true)]);
    let entry = state.get(&key).unwrap();
    assert!(entry.solicited);
    assert!(entry.latest().is_none());
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

/// A resume opens the entry again; the end it had not delivered stays owed
/// beside the resumed run, and a report marks only the end it names.
#[test]
fn a_reopened_run_keeps_its_unreported_end_owed() {
    let key = flow("f");
    let mut events = vec![owned(key.clone(), false), ended(key.clone(), 0)];
    events.push(RunLedgerEvent::Reopened {
        key: key.clone(),
        solicited: true,
        location: None,
        at_ms: Some(7),
    });
    let state = replay(&events);
    let entry = state.get(&key).unwrap();
    assert!(entry.solicited, "a reopen carries the run's solicitation");
    assert!(matches!(
        &entry.state,
        EntryState::Open { owed, since_ms: Some(7) } if owed.len() == 1 && owed[0].episode == 0
    ));
    assert_eq!(held(&state, u64::MAX), ["f"], "an open run is held");

    events.push(ended(key.clone(), 1));
    let state = replay(&events);
    let entry = state.get(&key).unwrap();
    assert_eq!(entry.unreported().unwrap().episode, 1);
    assert_eq!(owed_episodes(&state, &key), [0]);

    let mut older = events.clone();
    older.push(reported(key.clone(), 0));
    let entry = replay(&older).get(&key).unwrap().clone();
    assert!(entry.owed().is_empty());
    assert_eq!(entry.unreported().unwrap().episode, 1);

    events.push(reported(key.clone(), 1));
    let state = replay(&events);
    let entry = state.get(&key).unwrap();
    assert!(
        matches!(
            &entry.state,
            EntryState::Reported { end, .. } if end.episode == 1
        ),
        "the newer report leaves the owed end owed: {entry:?}"
    );
    assert_eq!(owed_episodes(&state, &key), [0]);
    assert_eq!(entry.unreported().unwrap().episode, 0);
    assert_eq!(held(&state, u64::MAX), ["f"], "an owed end keeps its hold");

    let mut resumed = events.clone();
    resumed.push(reopened(key.clone()));
    assert!(matches!(
        &replay(&resumed).get(&key).unwrap().state,
        EntryState::Open { .. }
    ));
    assert_eq!(owed_episodes(&replay(&resumed), &key), [0]);
    resumed.push(ended(key.clone(), 2));
    assert!(matches!(
        &replay(&resumed).get(&key).unwrap().state,
        EntryState::Unreported { end, .. } if end.episode == 2
    ));
    assert_eq!(owed_episodes(&replay(&resumed), &key), [0]);

    events.push(reported(key.clone(), 0));
    let state = replay(&events);
    assert!(matches!(
        &state.get(&key).unwrap().state,
        EntryState::Reported { end, owed } if end.episode == 1 && owed.is_empty()
    ));
    assert!(state.get(&key).unwrap().unreported().is_none());
    events.push(reopened(key.clone()));
    assert!(matches!(
        replay(&events).get(&key).unwrap().state,
        EntryState::Open { ref owed, .. } if owed.is_empty()
    ));
}

/// The cap evicts only what an open run owes: the run stays owned, and its
/// directory held, while it runs.
#[test]
fn an_eviction_drops_only_the_owed_end_of_an_open_run() {
    let open = flow("open");
    let mut events = vec![
        owned(open.clone(), true),
        ended(open.clone(), 0),
        reopened(open.clone()),
    ];
    for n in 1..=UNREPORTED_MAX as u64 {
        events.push(ended(task(&n.to_string()), n));
    }
    let state = replay(&events);
    let entry = state.get(&open).expect("the open run stays owned");
    assert!(
        matches!(&entry.state, EntryState::Open { owed, .. } if owed.is_empty()),
        "{entry:?}"
    );
    assert!(entry.solicited);
    assert!(held(&state, u64::MAX).contains(&"open".to_owned()));

    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    for event in &events {
        store.record(event.clone());
    }
    store.flush(Duration::from_secs(5)).unwrap();
    let answered = store.take_answered();
    assert!(answered.failure.is_none() && answered.rejected.is_none());
    assert_eq!(answered.evicted, vec![open.clone()], "counted as evicted");
    drop(store);
    let (_, reopened) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    assert_eq!(reopened, state);
}

/// A log written before resumes were modelled (schema 2: `Owned` without a
/// time) replays into today's entry states.
#[test]
fn a_schema_2_log_replays_into_the_current_state() {
    let dir = tempfile::tempdir().unwrap();
    let flow_f = r#"{"kind":"flow","run_id":"f"}"#;
    let flow_g = r#"{"kind":"flow","run_id":"g"}"#;
    let task_t = r#"{"kind":"task","task_id":"t","execution_id":"e1"}"#;
    let end = r#"{"what":"work","why":"because","ended_as":"completed","text":"answer","pointers":{"run_dir":"/run","report":null,"outputs":[],"chat":null,"transcript":null}}"#;
    let events = [
        format!(
            r#"{{"kind":"owned","key":{flow_f},"solicited":true,"location":{{"run_dir":"/tree/f","lock_dir":null}}}}"#
        ),
        format!(
            r#"{{"kind":"ended","key":{flow_f},"solicited":true,"episode":0,"end":{end},"evicts":[],"ended_at_ms":5}}"#
        ),
        format!(r#"{{"kind":"reported","key":{flow_f},"episode":0}}"#),
        format!(r#"{{"kind":"released","key":{flow_f},"reason":"reported"}}"#),
        format!(r#"{{"kind":"owned","key":{task_t},"solicited":false,"location":null}}"#),
        format!(
            r#"{{"kind":"ended","key":{task_t},"solicited":false,"episode":1,"end":{end},"evicts":[],"ended_at_ms":6}}"#
        ),
        format!(r#"{{"kind":"owned","key":{flow_g},"solicited":true}}"#),
    ];
    let mut lines = vec![
        r#"{"v":1,"seq":1,"at_ms":0,"kind":"opened","epoch":1,"build":"old","schema":2}"#
            .to_owned(),
    ];
    for (n, event) in events.iter().enumerate() {
        lines.push(format!(
            r#"{{"v":1,"seq":{},"at_ms":{n},"kind":"event","event":{event}}}"#,
            n + 2
        ));
    }
    let log = daruda_runlog::FileBackend::new(dir.path()).log_path(0);
    std::fs::write(&log, lines.join("\n") + "\n").unwrap();

    let (_store, state) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    let expected = replay(&[
        RunLedgerEvent::Owned {
            key: flow("f"),
            solicited: true,
            location: Some(RunLocation {
                run_dir: "/tree/f".into(),
                lock_dir: None,
            }),
            at_ms: None,
        },
        ended_at(flow("f"), 0, 5),
        reported(flow("f"), 0),
        released(flow("f"), ReleaseReason::Reported),
        owned(task("t"), false),
        ended_at(task("t"), 1, 6),
        owned(flow("g"), true),
    ]);
    assert_eq!(state, expected);
    assert!(state.get(&flow("f")).is_none());
    assert_eq!(
        state.get(&task("t")).unwrap().unreported().unwrap().episode,
        1
    );
    assert!(matches!(
        state.get(&flow("g")).unwrap().state,
        EntryState::Open {
            ref owed,
            since_ms: None
        } if owed.is_empty()
    ));
    assert_eq!(state.next_episode(), 2);
}

#[test]
fn reopening_a_released_run_owns_it_again() {
    let key = task("t");
    let state = replay(&[
        ended(key.clone(), 0),
        reported(key.clone(), 0),
        released(key.clone(), ReleaseReason::Reported),
        reopened(key.clone()),
    ]);
    let entry = state.get(&key).unwrap();
    assert!(entry.solicited);
    assert!(entry.latest().is_none());
}

#[test]
fn re_recording_an_owed_end_replaces_its_text() {
    let key = flow("f");
    let mut enriched = end("work");
    enriched.text = Some("the full report".into());
    let state = replay(&[
        ended(key.clone(), 0),
        reopened(key.clone()),
        RunLedgerEvent::Ended {
            key: key.clone(),
            solicited: false,
            episode: 0,
            end: enriched.clone(),
            evicts: Vec::new(),
            ended_at_ms: None,
        },
    ]);
    assert_eq!(state.get(&key).unwrap().owed()[0].end, enriched);
    assert_eq!(state.next_episode(), 1);
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
fn a_report_of_an_episode_no_end_has_used_is_rejected() {
    let key = task("t");
    let state = replay(&[owned(key.clone(), false)]);
    assert!(matches!(
        RunLedgerMachine::check(&state, &reported(key.clone(), 0)),
        Err(LedgerRejected::Unassigned { .. })
    ));
    let state = replay(&[owned(key.clone(), false), ended(key.clone(), 0)]);
    assert!(RunLedgerMachine::check(&state, &reported(key.clone(), 1)).is_err());
}

/// A report names its key's ends by episode: one of a generation the ledger
/// already let go (released, evicted, or resumed past) is a stale no-op, so
/// a receipt that lands after a release or a resume never stops the ledger.
#[test]
fn a_late_report_of_an_older_generation_is_accepted_and_changes_nothing() {
    let key = flow("f");
    let resumed = [
        owned(key.clone(), true),
        ended(key.clone(), 0),
        ended(key.clone(), 1),
        reported(key.clone(), 1),
        released(key.clone(), ReleaseReason::Reported),
        owned(key.clone(), true),
    ];
    let state = replay(&resumed);
    RunLedgerMachine::check(&state, &reported(key.clone(), 0)).unwrap();
    let mut after = state.clone();
    RunLedgerMachine::apply(&mut after, &reported(key, 0));
    assert_eq!(after, state);
}

#[test]
fn reports_and_releases_of_forgotten_keys_change_nothing() {
    let state = replay(&[owned(task("kept"), false), ended(task("other"), 4)]);
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
fn a_store_answers_which_entries_its_ends_evicted_once() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    for n in 0..=UNREPORTED_MAX as u64 {
        store.record(owned(task(&n.to_string()), false));
        store.record(ended(task(&n.to_string()), n));
    }
    store.flush(Duration::from_secs(5)).unwrap();
    let answered = store.take_answered();
    assert!(answered.failure.is_none());
    assert_eq!(answered.evicted, vec![task("0")]);
    assert!(
        store.take_answered().evicted.is_empty(),
        "each answer is read once"
    );
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
        at_ms: Some(1),
    };
    let events = vec![
        located,
        owned(task("t"), false),
        ended(key.clone(), 0),
        reopened(key.clone()),
        ended(task("t"), 1),
        ended(key.clone(), 2),
        reported(key.clone(), 0),
        reported(task("t"), 1),
        released(key.clone(), ReleaseReason::Reported),
        released(task("t"), ReleaseReason::Expired),
    ];
    let states = vec![
        RunLedgerState::default(),
        replay(&events[..3]),
        replay(&events[..4]),
        replay(&events[..6]),
        replay(&events[..8]),
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
fn a_crash_at_any_write_keeps_owed_ends_older_than_current_ones() {
    let key = task("t");
    let scenario = vec![
        owned(key.clone(), true),
        ended(key.clone(), 0),
        reported(key.clone(), 0),
        released(key.clone(), ReleaseReason::Reported),
        owned(flow("f"), false),
        ended(flow("f"), 1),
        reopened(flow("f")),
        ended(flow("f"), 2),
        reopened(flow("f")),
        ended(flow("f"), 3),
        reported(flow("f"), 1),
        reported(flow("f"), 3),
        reopened(flow("f")),
        reported(flow("f"), 2),
    ];
    let summary = enumerate_crashes_with::<RunLedgerMachine>(scenario, |case| {
        assert_ends_in_order(case.reopened.state());
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
    assert!(store.take_answered().failure.is_none());
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
    let answered = store.take_answered();
    assert!(matches!(
        answered.rejected,
        Some(LedgerRejected::Unassigned { .. })
    ));
    assert!(answered.failure.is_none(), "a refusal leaves the log open");
    assert!(store.take_answered().rejected.is_none());
    store.record(owned(task("later"), false));
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(store.take_answered().failure.is_none());
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
    assert!(store.take_answered().failure.is_none());
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

/// Two resumes with neither earlier end reported: both stay owed, oldest
/// first, and each report marks only its own end.
#[test]
fn every_end_a_run_owes_across_resumes_is_kept_until_its_own_report() {
    let key = flow("f");
    let mut events = vec![
        owned(key.clone(), true),
        ended(key.clone(), 0),
        reopened(key.clone()),
        ended(key.clone(), 1),
        reopened(key.clone()),
    ];
    let state = replay(&events);
    assert_eq!(owed_episodes(&state, &key), [0, 1]);
    assert_eq!(state.get(&key).unwrap().unreported().unwrap().episode, 1);
    assert_ends_in_order(&state);

    events.push(ended(key.clone(), 2));
    let state = replay(&events);
    let entry = state.get(&key).unwrap();
    let unreported: Vec<_> = entry.unreported_ends().map(|end| end.episode).collect();
    assert_eq!(unreported, [0, 1, 2]);

    events.push(reported(key.clone(), 1));
    events.push(reported(key.clone(), 2));
    let state = replay(&events);
    assert_eq!(owed_episodes(&state, &key), [0], "only the named ends");
    assert!(matches!(
        state.get(&key).unwrap().state,
        EntryState::Reported { .. }
    ));
    events.push(reported(key.clone(), 0));
    let state = replay(&events);
    assert!(state.get(&key).unwrap().unreported().is_none());
}

/// The cap counts every owed end and drops the oldest of them first, a run's
/// own included, recording each drop so none is lost silently.
#[test]
fn the_cap_evicts_owed_ends_oldest_first_and_records_each() {
    let key = flow("f");
    let mut events = vec![
        owned(key.clone(), true),
        ended(key.clone(), 0),
        reopened(key.clone()),
        ended(key.clone(), 1),
        reopened(key.clone()),
    ];
    // Two owed ends and these fill the cap exactly.
    for n in 2..UNREPORTED_MAX as u64 {
        events.push(ended(task(&n.to_string()), n));
    }
    assert_eq!(owed_episodes(&replay(&events), &key), [0, 1]);
    events.push(ended(task("next"), UNREPORTED_MAX as u64));
    let state = replay(&events);
    assert_eq!(owed_episodes(&state, &key), [1], "the oldest owed end went");
    assert!(state.get(&key).is_some(), "the running flow stays owned");

    // The run's own earlier ends are dropped too when they are the oldest.
    events.push(ended(key.clone(), UNREPORTED_MAX as u64 + 1));
    let state = replay(&events);
    assert!(owed_episodes(&state, &key).is_empty(), "{state:?}");
    assert_eq!(
        state.get(&key).unwrap().unreported().unwrap().episode,
        UNREPORTED_MAX as u64 + 1
    );
    let count: usize = state
        .entries()
        .iter()
        .map(|entry| entry.unreported_ends().count())
        .sum();
    assert_eq!(count, UNREPORTED_MAX);

    let dir = tempfile::tempdir().unwrap();
    let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    for event in &events {
        store.record(event.clone());
    }
    store.flush(Duration::from_secs(5)).unwrap();
    let answered = store.take_answered();
    assert!(answered.failure.is_none() && answered.rejected.is_none());
    assert_eq!(answered.evicted, vec![key.clone(), key.clone()]);
    drop(store);
    let (_, reopened) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    assert_eq!(reopened, state);
}

/// A schema 4 snapshot kept one owed end per entry, as `null` or an end, and
/// a schema 3 `reported` state had none: each reads as today's owed list.
#[test]
fn a_schema_4_state_reads_its_single_owed_end_as_a_list() {
    let end = r#"{"episode":0,"end":{"what":"work","why":"because","ended_as":"completed","text":"answer","pointers":{"run_dir":"/run","report":null,"outputs":[],"chat":null,"transcript":null}},"ended_at_ms":5}"#;
    let later = end.replace(r#""episode":0"#, r#""episode":1"#);
    let flow_f = r#"{"kind":"flow","run_id":"f"}"#;
    let flow_g = r#"{"kind":"flow","run_id":"g"}"#;
    let task_t = r#"{"kind":"task","task_id":"t","execution_id":"e1"}"#;
    let json = format!(
        r#"{{"next_episode":2,"entries":[
            {{"key":{flow_f},"solicited":true,"location":null,"state":{{"kind":"open","owed":{end},"since_ms":7}}}},
            {{"key":{flow_g},"solicited":false,"location":null,"state":{{"kind":"reported","end":{later},"owed":null}}}},
            {{"key":{task_t},"solicited":false,"state":{{"kind":"reported","end":{later}}}}}
        ]}}"#
    );
    let state: RunLedgerState = serde_json::from_str(&json).unwrap();
    assert_eq!(owed_episodes(&state, &flow("f")), [0]);
    assert!(matches!(
        state.get(&flow("f")).unwrap().state,
        EntryState::Open {
            since_ms: Some(7),
            ..
        }
    ));
    assert!(owed_episodes(&state, &flow("g")).is_empty());
    assert!(owed_episodes(&state, &task("t")).is_empty());
    let expected = replay(&[
        ended_at(flow("f"), 0, 5),
        RunLedgerEvent::Reopened {
            key: flow("f"),
            solicited: true,
            location: None,
            at_ms: Some(7),
        },
    ]);
    assert_eq!(state.get(&flow("f")), expected.get(&flow("f")));
}

/// A flow owing several ends holds its directory until the newest of their
/// holds ends, and for as long as any of them is undated.
#[test]
fn every_owed_end_of_a_flow_holds_its_directory() {
    let at = 100 * DAY_MS;
    let state = replay(&[
        ended_at(flow("dated"), 0, at),
        reopened(flow("dated")),
        ended_at(flow("dated"), 1, at + 3 * DAY_MS),
        reported(flow("dated"), 0),
        reopened(flow("dated")),
        ended_at(flow("dated"), 2, at + 5 * DAY_MS),
        reported(flow("dated"), 2),
        ended(flow("undated"), 3),
        reopened(flow("undated")),
        ended_at(flow("undated"), 4, at),
    ]);
    assert_eq!(owed_episodes(&state, &flow("dated")), [1]);
    assert_eq!(
        held(&state, at + 3 * DAY_MS + HOLD_MS - 1),
        ["dated", "undated"]
    );
    assert_eq!(held(&state, at + 3 * DAY_MS + HOLD_MS), ["undated"]);
}

/// A corrupt owed value names the field and the shapes it may take, not only
/// that no variant matched.
#[test]
fn a_corrupt_owed_value_names_what_was_expected() {
    let state = |owed: &str| {
        format!(
            r#"{{"next_episode":1,"entries":[{{"key":{{"kind":"flow","run_id":"f"}},"solicited":true,"state":{{"kind":"open","owed":{owed}}}}}]}}"#
        )
    };
    let error = serde_json::from_str::<RunLedgerState>(&state(r#""oops""#)).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("owed"), "{message}");
    assert!(message.contains("string \"oops\""), "{message}");
    assert!(message.contains("a list"), "{message}");
    let error = serde_json::from_str::<RunLedgerState>(&state("[{}]")).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("missing field `episode`"),
        "an element says what it lacks: {message}"
    );
}

/// A store told how holds change hears each dated hold of a located flow
/// as it is committed, and its end once the run is released.
#[test]
fn a_marking_store_tells_each_located_dated_hold_and_its_end() {
    use std::sync::{Arc, Mutex};
    let at = 100 * DAY_MS;
    let dir = tempfile::tempdir().unwrap();
    let told = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&told);
    let (store, _) = RunLedgerStore::marking(
        dir.path(),
        "test",
        Box::new(move |run_dir, until| {
            sink.lock().unwrap().push((run_dir.to_path_buf(), until));
        }),
    )
    .unwrap();
    let location = RunLocation {
        run_dir: "/runs/f".into(),
        lock_dir: None,
    };
    store.record(RunLedgerEvent::Owned {
        key: flow("f"),
        solicited: true,
        location: Some(location),
        at_ms: Some(at),
    });
    store.record(ended_at(flow("f"), 0, at));
    store.record(ended_at(flow("unlocated"), 1, at));
    store.record(reported(flow("f"), 0));
    store.record(released(flow("f"), ReleaseReason::Reported));
    store.flush(Duration::from_secs(5)).unwrap();
    assert_eq!(
        *told.lock().unwrap(),
        [
            (std::path::PathBuf::from("/runs/f"), Some(at + HOLD_MS)),
            (std::path::PathBuf::from("/runs/f"), None),
        ],
        "an open run holds with no deadline, and a run with no location is never told"
    );
}
