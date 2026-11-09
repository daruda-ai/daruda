//! Flow runs a restart finds on disk: interrupted, ended, still running or gone.

use super::*;

#[gpui::test]
fn a_flow_the_restart_interrupted_is_reported_resumable(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = finished_flow(&lane.path(), &lane.locks(), "interrupted");
    interrupt(&run_dir);
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        own_flow(lane.location(&run_dir), cx)
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert!(pending[0].solicited);
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Crashed));
        assert!(EndedAs::Crashed.resumable());
        let RunState::Ended {
            text: Some(text), ..
        } = &pending[0].snapshot.state
        else {
            panic!("an end with text: {:?}", pending[0].snapshot.state);
        };
        assert!(text.contains("1 node"), "how far it got: {text}");
        assert!(text.contains("resum"), "says it is resumable: {text}");
        assert!(RunsHost::run_recovered(&key, cx));
    });
}

#[gpui::test]
fn a_flow_that_ended_while_daruda_was_closed_is_delivered_from_disk(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = finished_flow(&lane.path(), &lane.locks(), "finished");
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        own_flow(lane.location(&run_dir), cx)
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    let report =
        std::fs::read_to_string(run_dir.join(daruda_flow::record::RUN_REPORT_FILE)).unwrap();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Completed));
        let RunState::Ended { text, .. } = &pending[0].snapshot.state else {
            unreachable!("checked above");
        };
        assert_eq!(
            text.as_deref(),
            daruda_control_types::agent_text::bound_agent_text(&report).as_deref()
        );
    });
}

#[gpui::test]
fn a_flow_another_process_still_runs_stays_pending(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = finished_flow(&lane.path(), &lane.locks(), "elsewhere");
    interrupt(&run_dir);
    let location = lane.location(&run_dir);
    let lock = daruda_flow::lock::RunLock::acquire(
        location.lock_dir.as_ref().unwrap(),
        "elsewhere",
        &daruda_core::process::is_alive,
    )
    .unwrap();
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        own_flow(location, cx)
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        // A later read finds it on disk, as a wait's refresh does.
        let read = RunsHost::run_query(key.clone(), cx).read();
        assert!(matches!(read.state, RunState::Running { .. }), "{read:?}");
    });
    lock.release().unwrap();
    cx.update(|cx| {
        let read = RunsHost::run_query(key, cx).read();
        assert!(
            matches!(
                read.state,
                RunState::Ended {
                    ended_as: EndedAs::Crashed,
                    ..
                }
            ),
            "{read:?}"
        );
    });
}

#[gpui::test]
fn a_flow_whose_directory_is_gone_is_released(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = crate::runs::runs_dir(&lane.path()).join("swept");
    let key = cx.update(|cx| {
        lane.start(&dirs, cx);
        own_flow(lane.location(&run_dir), cx)
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        let (id, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(id, false, cx).expect("not left waiting");
        assert_eq!(answer.outcome, WaitOutcome::Nothing);
        RunsHost::finish_run_wait(id, true, cx);
        assert!(matches!(
            RunsHost::run_query(key.clone(), cx).read().state,
            RunState::Gone
        ));
        RunsHost::flush_run_ledger_for_test(cx);
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| assert!(RunsHost::run_scope(cx).is_empty(), "released"));
}

/// A phone-asked orchestrator Task backed by a flow the restart interrupted.
/// Its task, still running, lives in `dirs`' log; its flow's run is returned.
fn interrupted_flow_task(
    dirs: &Dirs,
    lane: &Lane,
    cx: &mut TestAppContext,
) -> (String, RunKey, PathBuf) {
    use daruda_store::tasks::TaskExecutionSource;
    let run_id = "task-flow";
    let run = SeedRun {
        source: TaskExecutionSource::Flow {
            run_id: run_id.into(),
            flow: "restarted.yaml".into(),
        },
        ..SeedRun::chat(lane.path())
    };
    let (id, key) = cx.update(|cx| {
        lane.start(dirs, cx);
        let spec = seed::spec(Default::default(), "flow task");
        let id = spec.id.clone();
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
        (id, key)
    });
    let run_dir = finished_flow(&lane.path(), &lane.locks(), run_id);
    interrupt(&run_dir);
    cx.update(|cx| lane.restart(dirs, cx));
    cx.run_until_parked();
    (id, key, run_dir)
}

#[gpui::test]
async fn a_task_whose_flow_was_interrupted_reads_resumable_then_settles_after_a_resume(
    cx: &mut TestAppContext,
) {
    use daruda_store::tasks::LifecycleKind;
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let (id, key, run_dir) = interrupted_flow_task(&dirs, &lane, cx);
    let lifecycle = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.global::<GlobalTasks>()
                .get(&id)
                .unwrap()
                .lifecycle
                .kind()
        })
    };
    assert_eq!(lifecycle(cx), LifecycleKind::Running, "the task runs on");
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(pending[0].origin, Origin::Recovered);
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Crashed));
        let read = RunsHost::run_query(key.clone(), cx).read();
        assert!(
            matches!(
                read.state,
                RunState::Ended {
                    ended_as: EndedAs::Crashed,
                    ..
                }
            ),
            "not Running forever: {read:?}"
        );
        let (wait, _) = RunsHost::open_run_wait(None, cx).unwrap();
        assert_eq!(
            RunsHost::poll_run_wait(wait, false, cx)
                .unwrap()
                .ended
                .len(),
            1
        );
        RunsHost::finish_run_wait(wait, true, cx);
    });
    // Reported, still running: a second restart neither drops the task from
    // the scope nor reports the same interruption again.
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        assert!(RunsHost::unreported_run_events(cx).is_empty());
    });

    resume_and_finish(&lane, &run_dir, &key, cx).await;
    assert_eq!(lifecycle(cx), LifecycleKind::Done);
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Completed));
        assert_eq!(pending[0].origin, Origin::Live);
    });
}

/// A person resumes the run from the Flows panel; it ends, and so does its task.
async fn resume_and_finish(
    lane: &Lane,
    run_dir: &std::path::Path,
    key: &RunKey,
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        RunsHost::seed_flow_run(
            lane.path(),
            crate::runs::RunView::new(
                run_dir.to_path_buf(),
                crate::runs::FlowSource::Resumed {
                    run_dir: run_dir.to_path_buf(),
                },
                Vec::new(),
            ),
            cx,
        );
        RunsHost::flow_task_resumed(run_dir, cx);
        assert!(matches!(
            RunsHost::run_query(key.clone(), cx).read().state,
            RunState::Running { .. }
        ));
        RunsHost::flow_task_ended(
            run_dir,
            &daruda_flow::event::RunEnd::Done,
            false,
            false,
            Some("resumed and done".into()),
            cx,
        );
    });
    let barrier = cx.update(|cx| RunsHost::barrier(cx));
    barrier.await;
    cx.run_until_parked();
}

/// Without a readable task log nothing tells a reported, still-running Task
/// from a finished one: it is kept, so a later start neither reports the same
/// interruption again nor forgets who asked for the run.
#[gpui::test]
async fn an_unreadable_task_log_keeps_a_reported_task_in_scope(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let (_, key, run_dir) = interrupted_flow_task(&dirs, &lane, cx);
    cx.update(|cx| {
        let (wait, _) = RunsHost::open_run_wait(None, cx).unwrap();
        let answer = RunsHost::poll_run_wait(wait, false, cx).unwrap();
        assert_eq!(answer.ended.len(), 1);
        RunsHost::finish_run_wait(wait, true, cx);
        RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
        drop(cx.remove_global::<RunsHost>());
        drop(cx.remove_global::<GlobalTasks>());
        let unreadable = Err(daruda_store::tasks::TaskStoreError::Io(
            std::io::Error::other("unreadable"),
        ));
        let opened = RunsHost::open(unreadable)
            .with_run_ledger(RunLedgerStore::in_directory(dirs.ledger.path(), "test"))
            .with_flow_lock_root(lane.locks());
        RunsHost::install(opened, cx);
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        assert!(RunsHost::unreported_run_events(cx).is_empty());
    });
    cx.run_until_parked();
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(RunsHost::run_scope(cx), vec![key.clone()]);
        let pending = RunsHost::unreported_run_events(cx);
        assert!(pending.is_empty(), "reported once: {pending:?}");
    });
    resume_and_finish(&lane, &run_dir, &key, cx).await;
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert!(pending[0].solicited, "the phone still gets the real end");
    });
}

/// A crash between a flow's `Reported` and its `Released` leaves an entry with
/// nothing to deliver; the next start releases it instead of keeping it.
#[gpui::test]
fn a_reported_flow_whose_release_never_landed_is_released_on_restart(cx: &mut TestAppContext) {
    use daruda_store::run_ledger::{LedgerEnd, LedgerEndedAs, LedgerKey, RunLedgerEvent};
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let key = LedgerKey::Flow {
        run_id: "reported".into(),
    };
    {
        let (store, _) = RunLedgerStore::in_directory(dirs.ledger.path(), "test").unwrap();
        store.record(RunLedgerEvent::Owned {
            key: key.clone(),
            solicited: true,
            location: None,
            at_ms: None,
        });
        store.record(RunLedgerEvent::Ended {
            key: key.clone(),
            solicited: true,
            episode: 0,
            end: LedgerEnd {
                what: "Restarted".into(),
                why: None,
                ended_as: LedgerEndedAs::Completed,
                text: None,
                pointers: Default::default(),
            },
            evicts: Vec::new(),
            ended_at_ms: Some(now_ms()),
        });
        store.record(RunLedgerEvent::Reported {
            key: key.clone(),
            episode: 0,
        });
        store.flush(Duration::from_secs(5)).unwrap();
        assert!(store.take_answered().failure.is_none());
    }
    cx.update(|cx| {
        lane.start(&dirs, cx);
        assert!(RunsHost::run_scope(cx).is_empty());
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        RunsHost::flush_run_ledger_for_test(cx);
        drop(cx.remove_global::<RunsHost>());
    });
    let (_, state) = RunLedgerStore::in_directory(dirs.ledger.path(), "test").unwrap();
    assert!(state.get(&key).is_none(), "released on disk");
}

/// A Task whose flow's interruption was reported, then canceled in the
/// Tasks page without a resume: the cancel is the Task's real end.
#[gpui::test]
fn a_task_canceled_after_its_interruption_was_reported_delivers_the_cancel(
    cx: &mut TestAppContext,
) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let (_, key, _) = interrupted_flow_task(&dirs, &lane, cx);
    let RunKey::Task(exec) = key.clone() else {
        unreachable!("the fixture's key is a task");
    };
    cx.update(|cx| {
        let answer = report(None, cx);
        assert_eq!(answer.ended.len(), 1);
        seed::commit_now(
            vec![TaskEvent::CancelRequested {
                exec,
                at: chrono::Utc::now(),
            }],
            cx,
        );
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Canceled));
        assert!(pending[0].solicited);
        assert_eq!(report(None, cx).ended.len(), 1);
    });
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(RunsHost::unreported_run_events(cx).is_empty());
        assert!(RunsHost::run_scope(cx).is_empty(), "released");
    });
}

/// An orchestrator start scopes its run after `start_flow` answers, in the
/// same update; the run itself starts only once that record is durable, so
/// no crash can leave run output the ledger never owned.
#[gpui::test]
fn a_started_flow_runs_only_after_its_owner_is_on_disk(cx: &mut TestAppContext) {
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_dir = crate::runs::runs_dir(&lane.path()).join("01ORDER");
    let ledger_dir = dirs.ledger.path().to_path_buf();
    let (seen_tx, seen) = std::sync::mpsc::channel();
    cx.update(|cx| {
        lane.start(&dirs, cx);
        let request = command_flow(&lane.path(), &lane.locks(), &run_dir);
        let (events_tx, events) = smol::channel::unbounded();
        let (_, asks) = smol::channel::unbounded();
        let run_id = RunsHost::start_flow(
            crate::runs::FlowStart {
                tree: daruda_flow::lock::CanonicalTree::resolve(&lane.path()).unwrap(),
                request: daruda_flow::request::RunRequest {
                    events: Some(events_tx),
                    ..request
                },
                source: crate::runs::FlowSource::File(lane.path().join("flow.yaml")),
                runners: Box::new(move |request| {
                    // What the ledger holds on disk as the run begins.
                    let mut logged = String::new();
                    for file in std::fs::read_dir(&ledger_dir).unwrap() {
                        let path = file.unwrap().path();
                        if path.is_file() {
                            logged
                                .push_str(&String::from_utf8_lossy(&std::fs::read(path).unwrap()));
                        }
                    }
                    seen_tx.send(logged).unwrap();
                    command_runners(request)
                }),
                overage: Default::default(),
                submitter: crate::runs::FlowSubmitter::Orchestrator { solicited: true },
                events,
                asks,
            },
            cx,
        )
        .expect("the worktree is free");
        assert!(!run_dir.exists(), "the run waits for its caller's update");
        RunsHost::register_run_solicited(RunKey::Flow(run_id), true, cx);
    });
    let logged = seen.recv_timeout(Duration::from_secs(30)).unwrap();
    assert!(
        logged.contains(r#""kind":"owned""#) && logged.contains("01ORDER"),
        "{logged}"
    );
    wait_for_file(&run_dir.join("DONE"));
}

/// An unnamed flow that stalls: its first node removes the folder the
/// second works in, after the start checked that folder exists.
const UNNAMED_STALL: &str = "\
version: 1
nodes:
  - id: first
    kind: command
    run: \"rmdir sub\"
  - id: there
    kind: command
    deps: [first]
    cwd: sub
    run: \"true\"
";

/// A live end of an unnamed flow file, still owed when a resume the restart
/// cut off reopened it: the restart reads the same end from disk, which must
/// name the run as the live end did, or it is delivered a second time.
#[gpui::test]
async fn an_unnamed_flows_owed_end_is_not_repeated_after_a_cut_off_resume(cx: &mut TestAppContext) {
    // A real run thread wakes the app from outside the scheduler.
    cx.executor().allow_parking();
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let flows = lane.path().join(".daruda/flows");
    std::fs::create_dir_all(&flows).unwrap();
    std::fs::write(flows.join("ship.yaml"), UNNAMED_STALL).unwrap();
    std::fs::create_dir_all(lane.path().join("sub")).unwrap();
    let run_dir = crate::runs::runs_dir(&lane.path()).join("01STALL");
    let key = RunKey::flow_of(&run_dir);
    cx.update(|cx| {
        lane.start(&dirs, cx);
        let (events_tx, events) = smol::channel::unbounded();
        let (_, asks) = smol::channel::unbounded();
        let request = daruda_flow::request::RunRequest {
            history: daruda_flow::session::HistoryContext {
                source: Some(".daruda/flows/ship.yaml".into()),
                accounts: Default::default(),
            },
            loaded: daruda_flow::load(UNNAMED_STALL, None).unwrap(),
            events: Some(events_tx),
            ..command_flow(&lane.path(), &lane.locks(), &run_dir)
        };
        RunsHost::start_flow(
            crate::runs::FlowStart {
                tree: daruda_flow::lock::CanonicalTree::resolve(&lane.path()).unwrap(),
                request,
                source: crate::runs::FlowSource::File(flows.join("ship.yaml")),
                runners: Box::new(command_runners),
                overage: Default::default(),
                submitter: crate::runs::FlowSubmitter::Orchestrator { solicited: true },
                events,
                asks,
            },
            cx,
        )
        .expect("the worktree is free");
        RunsHost::register_run_solicited(key.clone(), true, cx);
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while cx.update(|cx| RunsHost::unreported_run_events(cx).is_empty()) {
        assert!(Instant::now() < deadline, "the live end never arrived");
        cx.background_executor
            .timer(Duration::from_millis(20))
            .await;
        cx.run_until_parked();
    }
    cx.update(|cx| {
        let live = RunsHost::unreported_run_events(cx);
        assert_eq!(ended_as(&live[0]), Some(EndedAs::Stalled), "{live:?}");
        // A resume the restart cuts off before it writes anything.
        on_watch(cx, |w| w.begin_with(&key, Some(true)));
        lane.restart(&dirs, cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "one end, delivered once: {pending:?}");
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Stalled));
        assert!(pending[0].resumable(), "the run can be picked up again");
    });
}
