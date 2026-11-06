use super::*;

/// Every way a run can end, and every pointer, comes back from the ledger
/// as it was recorded.
#[test]
fn an_end_round_trips_through_its_ledger_twin() {
    let key = RunKey::Flow("f".into());
    for ended_as in [
        EndedAs::Completed,
        EndedAs::Failed,
        EndedAs::Canceled,
        EndedAs::Stalled,
        EndedAs::Crashed,
        EndedAs::Interrupted,
        EndedAs::Undelivered,
        EndedAs::Error,
        EndedAs::BudgetExhausted,
    ] {
        let snapshot = RunSnapshot {
            key: key.clone(),
            what: "work".into(),
            why: Some("because".into()),
            state: RunState::Ended {
                ended_as,
                text: Some("answer".into()),
                pointers: Pointers {
                    run_dir: Some("/run".into()),
                    report: Some("/run/report.md".into()),
                    outputs: vec!["/run/out".into()],
                    chat: Some("chat".into()),
                    transcript: Some("/run/t.md".into()),
                },
            },
        };
        let end = ledger_end(&snapshot).unwrap();
        assert_eq!(snapshot_of(key_of(&ledger_key(&key)), &end), snapshot);
    }
    let running = RunSnapshot {
        key,
        what: String::new(),
        why: None,
        state: RunState::Unknown,
    };
    assert!(ledger_end(&running).is_none(), "only an end is recorded");
}
