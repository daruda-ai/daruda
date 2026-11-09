//! Ends owed from before a resume: delivered once, and the ledger keeps writing.

use super::*;

/// An end reported only after its flow was resumed belongs to an earlier
/// episode: the ledger keeps the resumed run owned, so a restart that
/// interrupts it again reports that interruption.
#[gpui::test]
fn a_flow_resumed_before_its_end_was_reported_stays_owned(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = finished_flow(&lane.path(), &lane.locks(), "resumed");
    interrupt(&run_dir);
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        own_flow(lane.location(&run_dir), cx)
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(RunsHost::unreported_run_events(cx).len(), 1);
        // The resume starts a new episode, as `start_flow` does.
        cx.update_global::<RunsHost, _>(|host, _| host.watch.begin_with(&key, Some(true)));
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        assert_eq!(answer.ended.len(), 1, "the earlier interruption");
        RunsHost::finish_run_wait(id, true, cx);
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Crashed));
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert!(pending[0].solicited);
    });
}

/// An earlier end reported after its run's newer end was reported, released
/// and the run resumed again names a generation the ledger let go.
#[gpui::test]
fn a_report_after_a_release_and_a_resume_keeps_the_ledger_writing(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = RunKey::Flow("f".into());
    cx.update(|cx| {
        dirs.start(cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "first")));
        on_watch(cx, |w| w.begin(&key));
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Completed, "second")));
        assert_eq!(whats(&report(Some(vec![key.clone()]), cx)), ["second"]);
        on_watch(cx, |w| w.begin(&key));
        assert_eq!(whats(&report(None, cx)), ["first"]);
        assert!(writing_after_flush(cx));
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
    });
}

/// One receipt covering two ends of a run that resumed again meanwhile.
#[gpui::test]
fn a_receipt_for_two_ends_of_a_resumed_run_keeps_the_ledger_writing(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = RunKey::Flow("f".into());
    cx.update(|cx| {
        dirs.start(cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "first")));
        on_watch(cx, |w| w.begin(&key));
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "second")));
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
        assert_eq!(whats(&answer), ["first", "second"]);
        on_watch(cx, |w| w.begin(&key));
        RunsHost::finish_run_wait(id, true, cx);
        assert!(writing_after_flush(cx));
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
    });
}

/// An end the cap evicted, reported after its run resumed.
#[gpui::test]
fn a_report_of_an_evicted_end_after_a_resume_keeps_the_ledger_writing(cx: &mut TestAppContext) {
    use daruda_store::run_ledger::UNREPORTED_MAX;
    let dirs = Dirs::new();
    let first = RunKey::Flow("0".into());
    cx.update(|cx| {
        dirs.start(cx);
        for n in 0..=UNREPORTED_MAX {
            let key = RunKey::Flow(n.to_string());
            RunsHost::register_run(key.clone(), cx);
            on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Completed, "end")));
        }
        on_watch(cx, |w| w.begin(&first));
        let answer = report(None, cx);
        assert_eq!(answer.ended[0].snapshot.key, first);
        assert!(writing_after_flush(cx));
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert!(pending.iter().all(|event| event.snapshot.key != first));
    });
}

/// A run resumed before its earlier end was reported, then a restart: the
/// earlier end is still owed, and the resumed run is read from disk.
#[gpui::test]
fn a_resume_keeps_the_earlier_end_owed_and_judges_the_new_run_after_a_restart(
    cx: &mut TestAppContext,
) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = finished_flow(&lane.path(), &lane.locks(), "resumed");
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        let key = own_flow(lane.location(&run_dir), cx);
        on_watch(cx, |w| {
            w.retain(end_of(&key, EndedAs::Crashed, "interrupted"))
        });
        on_watch(cx, |w| w.begin_with(&key, Some(true)));
        key
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        let ends: Vec<_> = pending.iter().map(ended_as).collect();
        assert_eq!(
            ends,
            [Some(EndedAs::Crashed), Some(EndedAs::Completed)],
            "{pending:?}"
        );
        assert!(
            pending
                .iter()
                .all(|event| event.origin == Origin::Recovered)
        );
        assert!(pending.iter().all(|event| event.solicited));
        assert_eq!(report(None, cx).ended.len(), 2);
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert!(RunsHost::run_scope(cx).is_empty(), "released with {key:?}");
    });
}

/// A desktop resume and a resume seen on disk keep who asked for the run.
#[gpui::test]
fn a_desktop_resume_keeps_the_runs_solicitation(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = crate::runs::runs_dir(&lane.path()).join("asked");
    cx.update(|cx| {
        lane.start(&dirs, cx);
        let key = own_flow(lane.location(&run_dir), cx);
        let request = command_flow(&lane.path(), &lane.locks(), &run_dir);
        let (events_tx, events) = smol::channel::unbounded();
        let (_, asks) = smol::channel::unbounded();
        RunsHost::start_flow(
            crate::runs::FlowStart {
                tree: daruda_flow::lock::CanonicalTree::resolve(&lane.path()).unwrap(),
                request: daruda_flow::request::RunRequest {
                    events: Some(events_tx),
                    ..request
                },
                source: crate::runs::FlowSource::Resumed {
                    run_dir: run_dir.clone(),
                },
                runners: Box::new(command_runners),
                overage: Default::default(),
                submitter: crate::runs::FlowSubmitter::Desktop,
                events,
                asks,
            },
            cx,
        )
        .expect("the worktree is free");
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Completed, "done")));
        let pending = RunsHost::unreported_run_events(cx);
        assert!(pending[0].solicited, "{pending:?}");
    });
    wait_for_file(&run_dir.join("DONE"));
}

/// An explicit wait takes only a run's current end. The end it owed from
/// before its resume is still owed after a restart, and only its own report
/// lets the run go.
#[gpui::test]
fn an_owed_end_outlives_a_report_of_the_newer_one_across_a_restart(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = RunKey::Flow("f".into());
    cx.update(|cx| {
        dirs.start(cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "first")));
        on_watch(cx, |w| w.begin(&key));
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Completed, "second")));
        assert_eq!(whats(&report(Some(vec![key.clone()]), cx)), ["second"]);
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.what, "first");
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        assert_eq!(whats(&report(None, cx)), ["first"]);
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert!(
            RunsHost::run_scope(cx).is_empty(),
            "released once both were"
        );
    });
}

/// One receipt handing over an owed end and the newer one reports both and
/// releases the run.
#[gpui::test]
fn one_receipt_for_an_owed_and_a_newer_end_releases_the_run(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = RunKey::Flow("f".into());
    cx.update(|cx| {
        dirs.start(cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "first")));
        on_watch(cx, |w| w.begin(&key));
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Completed, "second")));
        assert_eq!(whats(&report(None, cx)), ["first", "second"]);
        assert!(writing_after_flush(cx));
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert!(RunsHost::run_scope(cx).is_empty());
    });
}

/// An unreadable task log cannot judge a resumed Task's run, but the end it
/// owes from before the resume is still delivered, and the run stays owned.
#[gpui::test]
fn an_unreadable_task_log_still_delivers_a_resumed_tasks_owed_end(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = cx.update(|cx| {
        dirs.start(cx);
        let key = ended_task("owed", cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        on_watch(cx, |w| w.begin(&key));
        RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
        drop(cx.remove_global::<RunsHost>());
        drop(cx.remove_global::<GlobalTasks>());
        let unreadable = Err(daruda_store::tasks::TaskStoreError::Io(
            std::io::Error::other("unreadable"),
        ));
        let opened = RunsHost::open(unreadable)
            .with_run_ledger(RunLedgerStore::in_directory(dirs.ledger.path(), "test"));
        RunsHost::install(opened, cx);
        key
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert!(pending[0].solicited);
        let answer = report(None, cx);
        assert_eq!(
            answer.ended.len(),
            1,
            "a wait is not held by the unjudged run"
        );
        assert_eq!(answer.outcome, WaitOutcome::Settled);
        RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
        drop(cx.remove_global::<RunsHost>());
        drop(cx.remove_global::<GlobalTasks>());
    });
    let (_, state) = RunLedgerStore::in_directory(dirs.ledger.path(), "test").unwrap();
    let entry = state.get(&ledger_key(&key)).expect("the run stays owned");
    assert!(
        matches!(&entry.state, EntryState::Open { owed, .. } if owed.is_empty()),
        "{entry:?}"
    );
}

/// A Task key the task log never held, resumed after an end it still owes.
fn owed_unlisted_task(dirs: &Dirs, cx: &mut App) -> RunKey {
    dirs.start(cx);
    let spec = seed::spec(Default::default(), "deleted");
    let id = spec.id.clone();
    seed::commit_now(seed::created(spec), cx);
    let key = RunKey::Task(daruda_store::tasks::ExecutionRef {
        task_id: id,
        execution_id: "gone".into(),
    });
    RunsHost::register_run_solicited(key.clone(), true, cx);
    on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Completed, "owed")));
    on_watch(cx, |w| w.begin(&key));
    key
}

/// The entry the ledger on disk keeps for `key`, once this process is gone.
fn entry_on_disk(dirs: &Dirs, key: &RunKey, cx: &mut App) -> Option<RunLedgerEntry> {
    RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
    drop(cx.remove_global::<RunsHost>());
    drop(cx.remove_global::<GlobalTasks>());
    let (_, state) = RunLedgerStore::in_directory(dirs.ledger.path(), "test").unwrap();
    state.get(&ledger_key(key)).cloned()
}

/// A Task gone from the task log still owes the end it reached before its
/// resume: that end is delivered once, the gone run holds no wait, and its
/// entry is released once the end is reported.
#[gpui::test]
fn a_gone_task_delivers_what_it_owes_and_never_holds_a_wait(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = cx.update(|cx| owed_unlisted_task(&dirs, cx));
    cx.update(|cx| dirs.restart(cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.what, "owed");
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert_eq!(whats(&report(None, cx)), ["owed"]);

        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).expect("the gone run holds no wait");
        assert_eq!(answer.outcome, WaitOutcome::Nothing);
        RunsHost::finish_run_wait(id, true, cx);
        let (id, _) = RunsHost::open_run_wait(Some(vec![key.clone()]), cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).expect("an explicit wait is answered");
        assert_eq!(answer.outcome, WaitOutcome::Settled);
        RunsHost::finish_run_wait(id, true, cx);
        assert!(entry_on_disk(&dirs, &key, cx).is_none(), "released");
    });
}

/// A flow whose directory is gone still owes the end it reached before its
/// resume: a crash before that end is reported must not lose it.
#[gpui::test]
fn a_gone_flow_keeps_what_it_owes_until_it_is_reported(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = crate::runs::runs_dir(&lane.path()).join("swept");
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        let key = own_flow(lane.location(&run_dir), cx);
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "owed")));
        on_watch(cx, |w| w.begin(&key));
        key
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(RunsHost::unreported_run_events(cx).len(), 1);
        RunsHost::flush_run_ledger_for_test(cx);
        // Crash before anyone is handed the owed end.
        lane.start(&dirs, cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.what, "owed");
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert_eq!(whats(&report(None, cx)), ["owed"]);
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).expect("the gone run holds no wait");
        assert_eq!(answer.outcome, WaitOutcome::Nothing);
        RunsHost::finish_run_wait(id, true, cx);
        assert!(entry_on_disk(&dirs, &key, cx).is_none(), "released");
    });
}

/// Two resumes with neither earlier end reported: a restart owes both, each
/// delivered once, oldest first.
#[gpui::test]
fn every_end_owed_across_two_resumes_survives_a_restart(cx: &mut TestAppContext) {
    let dirs = Dirs::new();
    let key = RunKey::Flow("f".into());
    cx.update(|cx| {
        dirs.start(cx);
        RunsHost::register_run_solicited(key.clone(), true, cx);
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "first")));
        on_watch(cx, |w| w.begin(&key));
        on_watch(cx, |w| w.retain(end_of(&key, EndedAs::Crashed, "second")));
        on_watch(cx, |w| w.begin(&key));
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        let whats: Vec<_> = pending
            .iter()
            .map(|end| end.snapshot.what.as_str())
            .collect();
        assert_eq!(whats, ["first", "second"], "{pending:?}");
        assert!(pending.iter().all(|end| end.origin == Origin::Recovered));
        assert_eq!(report(None, cx).ended.len(), 2);
    });
    cx.update(|cx| dirs.restart(cx));
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()], "still running");
    });
}
