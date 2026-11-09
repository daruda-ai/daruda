//! Task ends and the ledger file itself across a restart.

use super::*;

#[gpui::test]
fn an_unreported_end_is_pending_again_after_a_restart(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = cx.update(|cx| {
        dirs.start(cx);
        let key = ended_task("ended", cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        assert_eq!(RunsHost::unreported_run_events(cx)[0].origin, Origin::Live);
        key
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "delivered once, not doubled by history");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert!(pending[0].solicited, "solicitation survives the restart");
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        assert!(RunsHost::run_recovered(&key, cx));

        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        assert_eq!(answer.ended.len(), 1);
        assert_eq!(answer.ended[0].origin, Origin::Recovered);
    });
}

#[gpui::test]
fn a_result_reported_before_a_restart_is_not_reported_again(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    cx.update(|cx| {
        dirs.start(cx);
        let key = ended_task("reported", cx);
        RunsHost::register_run(key, cx);
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        assert_eq!(
            RunsHost::poll_run_wait(id, false, cx).unwrap().ended.len(),
            1
        );
        RunsHost::finish_run_wait(id, true, cx);
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert!(RunsHost::run_scope(cx).is_empty());
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        assert_eq!(answer.outcome, WaitOutcome::Nothing);
    });
    // A second restart finds the ledger released, not merely skipped.
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| assert!(RunsHost::run_scope(cx).is_empty()));
}

#[gpui::test]
fn a_result_in_flight_at_a_crash_is_delivered_again_marked_recovered(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = cx.update(|cx| {
        dirs.start(cx);
        let key = ended_task("in flight", cx);
        RunsHost::register_run(key.clone(), cx);
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        assert_eq!(
            RunsHost::poll_run_wait(id, false, cx).unwrap().ended.len(),
            1
        );
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        // Wait for the ledger without quitting: the socket receipt never came.
        RunsHost::flush_run_ledger_for_test(cx);
        key
    });
    cx.update(|cx| dirs.crash_and_start(cx));
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(pending[0].origin, Origin::Recovered);
    });
}

#[gpui::test]
fn an_unreadable_ledger_leaves_delivery_in_memory(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let log = daruda_runlog::FileBackend::new(dirs.ledger.path()).log_path(0);
    std::fs::write(&log, b"not json\n{}\n").unwrap();
    let refused = RunLedgerStore::in_directory(dirs.ledger.path(), "test").unwrap_err();
    assert_eq!(
        open_failure(&refused).dedup_key.as_deref(),
        Some("run_ledger.open")
    );
    cx.update(|cx| {
        dirs.start(cx);
        assert!(!RunsHost::run_ledger_writing_for_test(cx));
        assert!(RunsHost::is_writable(cx), "tasks stay writable");
        let key = ended_task("memory", cx);
        RunsHost::register_run(key.clone(), cx);
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].origin, Origin::Live);
    });
    assert_eq!(std::fs::read(&log).unwrap(), b"not json\n{}\n");
}

#[test]
fn an_eviction_is_logged_by_kind_without_naming_runs() {
    use daruda_store::run_ledger::LedgerKey;
    let evicted = [
        LedgerKey::Flow {
            run_id: "secret-run".into(),
        },
        LedgerKey::Task(daruda_store::tasks::ExecutionRef {
            task_id: "secret-task".into(),
            execution_id: "secret-exec".into(),
        }),
        LedgerKey::Flow {
            run_id: "other".into(),
        },
    ];
    let report = eviction(&evicted);
    assert_eq!(report.dedup_key.as_deref(), Some("run_ledger.evicted"));
    assert_eq!(report.severity, ErrorSeverity::Warning);
    assert_eq!(report.context["evicted"], "3");
    assert_eq!(report.context["flows"], "2");
    assert_eq!(report.context["tasks"], "1");
    let rendered = format!("{report:?}");
    assert!(!rendered.contains("secret"), "{rendered}");
}

#[gpui::test]
fn a_carried_orchestrator_task_keeps_its_ledger_solicitation(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = cx.update(|cx| {
        dirs.start(cx);
        let spec = seed::spec(Default::default(), "running");
        let id = spec.id.clone();
        let run = SeedRun::chat("/wt");
        let mut events = seed::created(spec);
        events.extend(seed::run(&id, &run, SeedEnd::Running));
        for event in &mut events {
            if let TaskEvent::DispatchRequested { started_by, .. } = event {
                *started_by = Some(StartedBy::Orchestrator);
            }
        }
        seed::commit_now(events, cx);
        let key = RunKey::Task(run.exec(&id));
        RunsHost::register_run_solicited(key.clone(), true, cx);
        key
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        // Startup reconciliation ended the lost run; its end is announced once.
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].snapshot.key, key);
        assert!(
            pending[0].solicited,
            "the carry took the ledger's solicitation"
        );
        assert_eq!(
            pending[0].origin,
            Origin::Recovered,
            "it predates this process"
        );
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        let recovered: Vec<_> = answer
            .ended
            .iter()
            .filter(|end| end.origin == Origin::Recovered)
            .map(|end| end.snapshot.key.clone())
            .collect();
        assert_eq!(recovered, vec![key.clone()]);
    });
}

#[gpui::test]
fn a_deleted_task_with_nothing_to_deliver_leaves_the_ledger(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let id = cx.update(|cx| {
        dirs.start(cx);
        let spec = seed::spec(Default::default(), "deleted");
        let id = spec.id.clone();
        seed::commit_now(seed::created(spec), cx);
        RunsHost::register_run(
            RunKey::Task(daruda_store::tasks::ExecutionRef {
                task_id: id.clone(),
                execution_id: "never-started".into(),
            }),
            cx,
        );
        id
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::run_scope(cx).is_empty(), "{id} expired");
        dirs.restart(cx);
        assert!(RunsHost::run_scope(cx).is_empty());
    });
}

/// A record naming an episode no end has used is the proposer's bug: it is
/// logged as an error, and the records after it still land.
#[test]
fn a_refused_record_is_logged_and_the_ledger_keeps_writing() {
    use daruda_store::run_ledger::{LedgerKey, LedgerRejected};
    let dir = tempfile::tempdir().unwrap();
    let (mut ledger, _) = Ledger::open(RunLedgerStore::in_directory(dir.path(), "test"));
    let key = RunKey::Flow("f".into());
    ledger.reported(&key, 99);
    ledger.flush(Instant::now() + Duration::from_secs(5));
    assert!(matches!(ledger, Ledger::Writing { .. }));
    ledger.owned(&key, true, None);
    ledger.flush(Instant::now() + Duration::from_secs(5));
    assert!(matches!(ledger, Ledger::Writing { .. }));
    drop(ledger);
    let (_, state) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
    let flow = LedgerKey::Flow { run_id: "f".into() };
    assert!(state.get(&flow).is_some(), "the later record landed");

    let report = rejection(&LedgerRejected::Unassigned {
        key: flow,
        episode: 99,
    });
    assert_eq!(report.severity, ErrorSeverity::Error);
    assert_eq!(report.dedup_key.as_deref(), Some("run_ledger.rejected"));
}
