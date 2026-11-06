use super::super::{LedgerEnd, LedgerEndedAs, LedgerPointers};
use super::*;
use crate::tasks::ExecutionRef;

fn task(id: &str) -> LedgerKey {
    LedgerKey::Task(ExecutionRef {
        task_id: id.to_owned(),
        execution_id: "e".into(),
    })
}

fn with_end(state: &mut RunLedgerState, key: LedgerKey, episode: u64) {
    let end = RunLedgerEvent::Ended {
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
    };
    RunLedgerMachine::apply(state, &end);
}

/// Eviction order follows episodes, not the order keys were first recorded,
/// and counts every unreported end, a key's owed ones included.
#[test]
fn the_oldest_end_is_chosen_by_episode() {
    let mut state = RunLedgerState::default();
    for n in 1..UNREPORTED_MAX as u64 {
        with_end(&mut state, task(&n.to_string()), n);
    }
    assert!(state.evictions().is_empty(), "the next end fits");
    // The first key ends again: its earlier end stays owed, and the oldest.
    with_end(&mut state, task("1"), UNREPORTED_MAX as u64);
    assert_eq!(state.get(&task("1")).unwrap().owed().len(), 1);
    assert_eq!(state.evictions(), vec![task("1")], "its own owed end");
}
