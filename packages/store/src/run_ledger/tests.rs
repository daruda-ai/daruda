use super::*;
use crate::tasks::ExecutionRef;
use daruda_runlog::Machine as _;

fn task(id: &str) -> LedgerKey {
    LedgerKey::Task(ExecutionRef {
        task_id: id.to_owned(),
        execution_id: "e".into(),
    })
}

fn end_event(key: LedgerKey, episode: u64) -> RunLedgerEvent {
    RunLedgerEvent::Ended {
        key,
        solicited: false,
        episode,
        end: LedgerEnd {
            what: String::new(),
            why: None,
            ended_as: LedgerEndedAs::Completed,
            text: None,
            pointers: LedgerPointers::default(),
        },
        evicts: Vec::new(),
        ended_at_ms: None,
    }
}

/// Past its record budget the ledger starts a new generation on its own, and
/// reopening it replays to the state the records committed, resumed runs and
/// the ends they still owe included.
#[test]
fn the_ledger_compacts_past_its_budget_and_replays_the_same_state() {
    use daruda_runlog::{Compaction, FileBackend, LogBackend as _};
    let dir = tempfile::tempdir().unwrap();
    let (store, _) =
        RunLedgerStore::compacting(dir.path(), "test", Compaction::AfterRecords(8)).unwrap();
    let mut state = RunLedgerState::default();
    let record = |event: RunLedgerEvent, state: &mut RunLedgerState| {
        store.record(event.clone());
        RunLedgerMachine::apply(state, &event);
    };
    let mut episode = 0;
    for n in 0..12u64 {
        let key = task(&n.to_string());
        let owned = RunLedgerEvent::Owned {
            key: key.clone(),
            solicited: n % 2 == 0,
            location: None,
            at_ms: Some(n),
        };
        record(owned, &mut state);
        record(end_event(key.clone(), episode), &mut state);
        episode += 1;
        if n % 3 == 0 {
            let reported = RunLedgerEvent::Reported {
                key: key.clone(),
                episode: episode - 1,
            };
            record(reported, &mut state);
        }
        if n % 4 == 1 {
            let reopened = RunLedgerEvent::Reopened {
                key: key.clone(),
                solicited: true,
                location: None,
                at_ms: Some(100 + n),
            };
            record(reopened, &mut state);
            if n % 8 == 1 {
                // Ends again and only the newer end is reported.
                record(end_event(key.clone(), episode), &mut state);
                let reported = RunLedgerEvent::Reported { key, episode };
                record(reported, &mut state);
                episode += 1;
            }
        }
    }
    let states: Vec<_> = state.entries().iter().map(|entry| &entry.state).collect();
    assert!(
        states
            .iter()
            .any(|state| matches!(state, EntryState::Open { owed, .. } if !owed.is_empty())),
        "{states:?}"
    );
    assert!(
        states
            .iter()
            .any(|state| matches!(state, EntryState::Reported { owed, .. } if !owed.is_empty())),
        "{states:?}"
    );
    store.flush(std::time::Duration::from_secs(5)).unwrap();
    let answered = store.take_answered();
    assert!(answered.failure.is_none() && answered.rejected.is_none());
    drop(store);
    let generations = FileBackend::new(dir.path()).generations().unwrap();
    assert!(generations[0] > 0, "never compacted: {generations:?}");
    let (_, reopened) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    assert_eq!(reopened, state);
}
