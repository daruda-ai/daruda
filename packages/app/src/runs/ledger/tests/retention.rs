//! Directory holds: what the ledger keeps from the retention sweep, and for how long.

use super::*;

/// Finished runs newer than any other in `lane`, as earlier starts left
/// them: enough that the cap alone would sweep everything older.
fn newer_finished_runs(lane: &std::path::Path) -> PathBuf {
    let runs_dir = crate::runs::runs_dir(lane);
    for i in 0..daruda_flow::marker::DEFAULT_KEEP_RUNS + 5 {
        let run_dir = runs_dir.join(format!("01B{i:02}"));
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("DONE"), "").unwrap();
    }
    runs_dir
}

/// The sweep a run starting in `lane` at `now_ms` performs.
fn sweep_at(lane: &std::path::Path, now_ms: u64, cx: &App) -> Vec<PathBuf> {
    daruda_flow::marker::sweep_old_runs(
        &crate::runs::runs_dir(lane),
        daruda_flow::marker::DEFAULT_KEEP_RUNS,
        &RunsHost::held_flow_runs(now_ms, cx),
    )
    .unwrap()
}

/// An orchestrator flow that finished while daruda was closed, older than
/// every run [`newer_finished_runs`] adds: its end is recorded, unreported.
fn unreported_flow(dirs: &Dirs, lane: &Lane, cx: &mut TestAppContext) -> (RunKey, PathBuf) {
    let run_dir = finished_flow(&lane.path(), &lane.locks(), "01A00");
    let key = cx.update(|cx| {
        lane.start(dirs, cx);
        own_flow(lane.location(&run_dir), cx)
    });
    cx.update(|cx| lane.restart(dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        RunsHost::flush_run_ledger_for_test(cx);
    });
    (key, run_dir)
}

#[gpui::test]
fn an_unreported_flow_end_survives_the_sweep_a_newer_start_runs(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let (key, held) = unreported_flow(&dirs, &lane, cx);
    let runs_dir = newer_finished_runs(&lane.path());
    let next = runs_dir.join("01C00");
    cx.update(|cx| {
        let request = command_flow(&lane.path(), &lane.locks(), &next);
        let (events_tx, events) = smol::channel::unbounded();
        let (_, asks) = smol::channel::unbounded();
        let request = daruda_flow::request::RunRequest {
            events: Some(events_tx),
            ..request
        };
        RunsHost::start_flow(
            crate::runs::FlowStart {
                tree: daruda_flow::lock::CanonicalTree::resolve(&lane.path()).unwrap(),
                request,
                source: crate::runs::FlowSource::File(lane.path().join("flow.yaml")),
                runners: Box::new(command_runners),
                overage: Default::default(),
                submitter: crate::runs::FlowSubmitter::Desktop,
                events,
                asks,
            },
            cx,
        )
        .expect("the worktree is free");
    });
    wait_for_file(&next.join("DONE"));

    assert!(held.is_dir(), "the undelivered run was swept");
    assert!(!runs_dir.join("01B04").exists(), "the cap still applies");
    assert!(runs_dir.join("01B05").is_dir());
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
    });
}

#[gpui::test]
fn a_reported_flow_end_leaves_its_directory_to_the_sweep(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let (key, held) = unreported_flow(&dirs, &lane, cx);
    newer_finished_runs(&lane.path());
    cx.update(|cx| {
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        assert_eq!(answer.ended.len(), 1);
        assert_eq!(answer.ended[0].snapshot.key, key);
        RunsHost::finish_run_wait(id, true, cx);
        RunsHost::flush_run_ledger_for_test(cx);
        assert!(sweep_at(&lane.path(), now_ms(), cx).contains(&held));
    });
}

#[gpui::test]
fn an_unreported_flow_end_frees_its_directory_after_the_hold_but_is_still_delivered(
    cx: &mut TestAppContext,
) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let (key, held) = unreported_flow(&dirs, &lane, cx);
    newer_finished_runs(&lane.path());
    let hold_ms = u64::try_from(daruda_store::run_ledger::DIRECTORY_HOLD.as_millis()).unwrap();
    cx.update(|cx| {
        let now = now_ms();
        assert!(!sweep_at(&lane.path(), now, cx).contains(&held));
        assert!(held.is_dir());
        assert_eq!(
            sweep_at(&lane.path(), now + hold_ms, cx),
            vec![held.clone()]
        );
    });
    // The summary outlives the directory, across a restart too.
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        assert_eq!(answer.ended.len(), 1);
        assert_eq!(answer.ended[0].snapshot.key, key);
        assert_eq!(answer.ended[0].origin, Origin::Recovered);
    });
}

/// A write failure stops the ledger, but not the directory holds it had
/// committed: an owed flow's directory still survives retention.
#[test]
fn a_stopped_ledger_keeps_answering_its_committed_holds() {
    let dir = tempfile::tempdir().unwrap();
    let (mut ledger, _) = Ledger::open(RunLedgerStore::in_directory(dir.path(), "test"));
    let key = RunKey::Flow("owed".into());
    ledger.owned(&key, true, None);
    ledger.flush(Instant::now() + Duration::from_secs(5));
    ledger.observe(daruda_store::run_ledger::Answered {
        failure: Some(daruda_runlog::CommitError::Closed),
        ..Default::default()
    });
    assert!(matches!(ledger, Ledger::Stopped { .. }));
    assert!(ledger.held_run_dirs(now_ms()).contains("owed"));
}

/// Seed the ledger with an owned flow the process cannot locate.
fn owned_unlocated(dirs: &Dirs, run_id: &str, at_ms: u64) {
    use daruda_store::run_ledger::{LedgerKey, RunLedgerEvent};
    let (store, _) = RunLedgerStore::in_directory(dirs.ledger.path(), "test").unwrap();
    store.record(RunLedgerEvent::Owned {
        key: LedgerKey::Flow {
            run_id: run_id.into(),
        },
        solicited: true,
        location: None,
        at_ms: Some(at_ms),
    });
    store.flush(Duration::from_secs(5)).unwrap();
    assert!(store.take_answered().failure.is_none());
}

/// An owned flow no read can judge is kept for the directory hold, then let
/// go with a log line; nothing on disk but the ledger changes.
#[gpui::test]
fn an_unjudgeable_owned_flow_is_released_after_the_hold(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let hold_ms = u64::try_from(daruda_store::run_ledger::DIRECTORY_HOLD.as_millis()).unwrap();
    owned_unlocated(&dirs, "lost", now_ms() - hold_ms - 1);
    owned_unlocated(&dirs, "recent", now_ms());
    cx.update(|cx| dirs.start(cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(RunsHost::run_scope(cx), vec![RunKey::Flow("recent".into())]);
        dirs.restart(cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            RunsHost::run_scope(cx),
            vec![RunKey::Flow("recent".into())],
            "released on disk"
        );
    });
}

/// An end recorded before ends were dated holds its directory from when a
/// process first restores it, for the hold and no longer; a later restore or
/// a re-record of its text never moves that date.
#[test]
fn an_undated_end_is_dated_once_by_the_first_restore() {
    use daruda_store::run_ledger::{DIRECTORY_HOLD, LedgerKey, RunLedgerEvent};
    let dir = tempfile::tempdir().unwrap();
    let key = RunKey::Flow("old".into());
    let ledger_key = LedgerKey::Flow {
        run_id: "old".into(),
    };
    let snapshot = end_of(&key, EndedAs::Completed, "old work");
    {
        let (store, _) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
        store.record(RunLedgerEvent::Ended {
            key: ledger_key.clone(),
            solicited: true,
            episode: 0,
            end: ledger_end(&snapshot).unwrap(),
            evicts: Vec::new(),
            ended_at_ms: None,
        });
        store.flush(Duration::from_secs(5)).unwrap();
    }
    let dated_at = |rerecord: bool| {
        let (mut ledger, entries) = Ledger::open(RunLedgerStore::in_directory(dir.path(), "test"));
        assert_eq!(ledger.restore(entries, None).len(), 1);
        if rerecord {
            let mut enriched = snapshot.clone();
            if let RunState::Ended { text, .. } = &mut enriched.state {
                *text = Some("read later".into());
            }
            ledger.rerecord(0, &enriched, true);
        }
        ledger.flush(Instant::now() + Duration::from_secs(5));
        drop(ledger);
        let (_, state) = RunLedgerStore::in_directory(dir.path(), "test").unwrap();
        let end = state.get(&ledger_key).unwrap().latest().unwrap().clone();
        (end.ended_at_ms, state)
    };
    let before = now_ms();
    let (first, state) = dated_at(false);
    let first = first.expect("the first restore dates the end");
    assert!(first >= before);
    let hold = u64::try_from(DIRECTORY_HOLD.as_millis()).unwrap();
    assert!(state.held_run_dirs(first + hold - 1).contains("old"));
    assert!(state.held_run_dirs(first + hold).is_empty());
    std::thread::sleep(Duration::from_millis(5));
    assert_eq!(dated_at(true).0, Some(first), "kept by a later restore");
}

/// Another profile's sweep never sees this ledger, only the run directory:
/// an owed end's hold marker spares the run there until the end is reported.
#[test]
fn another_profiles_sweep_spares_a_run_this_ledger_owes() {
    let ledger_dir = tempfile::tempdir().unwrap();
    let lane = tempfile::tempdir().unwrap();
    let runs_dir = newer_finished_runs(lane.path());
    let run_dir = runs_dir.join("01A00");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join("DONE"), "").unwrap();
    let (mut ledger, _) = Ledger::open(RunLedgerStore::marking(
        ledger_dir.path(),
        "test",
        hold_markers(),
    ));
    let key = RunKey::Flow("01A00".into());
    let location = FlowRunLocation {
        run_dir: run_dir.clone(),
        lock_dir: None,
    };
    ledger.owned(&key, true, Some(&location));
    let episode = ledger
        .ended(&end_of(&key, EndedAs::Completed, "work"), true)
        .unwrap();
    ledger.flush(Instant::now() + Duration::from_secs(5));
    let other_profile =
        |keep| daruda_flow::marker::sweep_old_runs(&runs_dir, keep, &Default::default()).unwrap();
    assert!(!other_profile(daruda_flow::marker::DEFAULT_KEEP_RUNS).contains(&run_dir));
    assert!(
        run_dir.is_dir(),
        "an owed run survives a sweep that cannot see the ledger"
    );

    ledger.reported(&key, episode);
    ledger.settled(&key);
    ledger.flush(Instant::now() + Duration::from_secs(5));
    assert!(
        other_profile(0).contains(&run_dir),
        "a released run is left to retention again"
    );
}
