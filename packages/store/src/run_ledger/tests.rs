use super::*;

fn task(id: &str) -> LedgerKey {
    LedgerKey::Task(ExecutionRef {
        task_id: id.to_owned(),
        execution_id: "e".into(),
    })
}

fn with_end(state: &mut RunLedgerState, key: LedgerKey, episode: u64) {
    RunLedgerMachine::apply(
        state,
        &RunLedgerEvent::Ended {
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
        },
    );
}

/// Eviction order follows episodes, not the order keys were first recorded.
#[test]
fn the_oldest_end_is_chosen_by_episode() {
    let mut state = RunLedgerState::default();
    for n in 0..UNREPORTED_MAX as u64 {
        with_end(&mut state, task(&n.to_string()), n);
    }
    // The first key ends again, so its end is now the newest.
    with_end(&mut state, task("0"), UNREPORTED_MAX as u64);
    assert_eq!(state.evictions(&task("new")), vec![task("1")]);
    assert!(
        state.evictions(&task("0")).is_empty(),
        "a key replaces its own end"
    );
}
