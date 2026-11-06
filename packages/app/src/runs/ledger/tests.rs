//! Restart scenarios: a host is dropped and reopened from the same directories.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use daruda_store::run_ledger::RunLedgerStore;
use daruda_store::tasks::machine::StartedBy;
use daruda_store::tasks::{TaskEvent, TaskStore};
use gpui::{App, BorrowAppContext as _, TestAppContext};

use super::*;
use crate::runs::result::{EndedAs, RunState};
use crate::runs::seed::{self, SeedEnd, SeedRun};
use crate::runs::watch::{Origin, WaitOutcome};
use crate::runs::{GlobalTasks, RunsHost, no_hook};
use daruda_store::run_ledger::EntryState;

struct Dirs {
    tasks: tempfile::TempDir,
    ledger: tempfile::TempDir,
}

impl Dirs {
    fn new() -> Self {
        Self {
            tasks: tempfile::tempdir().unwrap(),
            ledger: tempfile::tempdir().unwrap(),
        }
    }

    fn start(&self, cx: &mut App) {
        let opened = RunsHost::open(TaskStore::in_directory(
            self.tasks.path(),
            None,
            "test",
            no_hook(),
        ))
        .with_run_ledger(RunLedgerStore::in_directory(self.ledger.path(), "test"));
        RunsHost::install(opened, cx);
    }

    /// Quit as the app does, then start a new process's host from disk.
    fn restart(&self, cx: &mut App) {
        RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
        self.crash_and_start(cx);
    }

    /// Drop the process's host without flushing anything first.
    fn crash_and_start(&self, cx: &mut App) {
        drop(cx.remove_global::<RunsHost>());
        drop(cx.remove_global::<GlobalTasks>());
        self.start(cx);
    }
}

fn ended_task(title: &str, cx: &mut App) -> RunKey {
    let run = SeedRun::chat("/wt");
    let task = seed::task(
        seed::spec(Default::default(), title),
        Some((&run, SeedEnd::Done)),
        cx,
    );
    RunKey::Task(run.exec(&task))
}

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

/// A one-node flow, run to its end in `lane` by the real engine: its log,
/// marker and `run.md` are what a run that finished leaves.
fn finished_flow(lane: &std::path::Path, locks: &std::path::Path, run_id: &str) -> PathBuf {
    let run_dir = crate::runs::runs_dir(lane).join(run_id);
    let request = command_flow(lane, locks, &run_dir);
    let runners = command_runners(&request);
    let report = daruda_flow::schedule::execute(
        &request,
        &runners,
        &daruda_flow::runner::CancelToken::default(),
    );
    assert!(
        matches!(report.outcome, daruda_flow::schedule::RunOutcome::Done),
        "{report:?}"
    );
    run_dir
}

/// A one-command flow that runs in `lane` and records into `run_dir`.
fn command_flow(
    lane: &std::path::Path,
    locks: &std::path::Path,
    run_dir: &std::path::Path,
) -> daruda_flow::request::RunRequest {
    let text =
        "version: 1\nname: Restarted\nnodes:\n  - id: ok\n    kind: command\n    run: \"exit 0\"\n";
    daruda_flow::request::RunRequest {
        history: Default::default(),
        started_by: Some(daruda_flow::request::StartedBy::Orchestrator),
        loaded: daruda_flow::load(text, None).unwrap(),
        cwd: lane.to_path_buf(),
        run_dir: run_dir.to_path_buf(),
        lock_dir: locks.to_path_buf(),
        flow_dir: lane.to_path_buf(),
        agents: Default::default(),
        node_install_dir: lane.join("node"),
        budget: daruda_flow::request::Budget::unlimited(),
        is_alive: Box::new(|_| false),
        git_status: None,
        events: None,
        ask: None,
        until: None,
        pinned: Vec::new(),
        inputs: Default::default(),
        resume: None,
        held_runs: Default::default(),
    }
}

fn command_runners(request: &daruda_flow::request::RunRequest) -> daruda_flow::runner::Runners {
    daruda_flow::runner::Runners {
        command: daruda_flow::runner::ProcessRunner::new(Vec::new()),
        agent: daruda_flow::runner::AcpRunner::new(
            Default::default(),
            request.node_install_dir.clone(),
        ),
    }
}

/// Make a finished run read as one a quit cut short: the log loses its end,
/// and the marker and report an end writes are gone. Its lock is free.
fn interrupt(run_dir: &std::path::Path) {
    let log = daruda_flow::run_log::log_file(run_dir);
    let text = std::fs::read_to_string(&log).unwrap();
    let kept: String = text
        .lines()
        .take_while(|line| !line.contains("\"run_ended\""))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(kept, text, "the fixture's log recorded an end");
    std::fs::write(&log, kept).unwrap();
    std::fs::remove_file(run_dir.join("DONE")).unwrap();
    std::fs::remove_file(run_dir.join(daruda_flow::record::RUN_REPORT_FILE)).unwrap();
}

/// A lane and the shared lock root its flows run under.
struct Lane {
    dir: tempfile::TempDir,
}

impl Lane {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("lane")).unwrap();
        Self { dir }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().join("lane")
    }

    fn locks(&self) -> PathBuf {
        self.dir.path().join("locks")
    }

    fn location(&self, run_dir: &std::path::Path) -> FlowRunLocation {
        let tree = daruda_flow::lock::CanonicalTree::resolve(&self.path()).unwrap();
        FlowRunLocation {
            run_dir: run_dir.to_path_buf(),
            lock_dir: Some(daruda_flow::lock::lock_dir_for(&self.locks(), &tree)),
        }
    }

    fn start(&self, dirs: &Dirs, cx: &mut App) {
        if cx.has_global::<RunsHost>() {
            drop(cx.remove_global::<RunsHost>());
            drop(cx.remove_global::<GlobalTasks>());
        }
        let opened = RunsHost::open(TaskStore::in_directory(
            dirs.tasks.path(),
            None,
            "test",
            no_hook(),
        ))
        .with_run_ledger(RunLedgerStore::in_directory(dirs.ledger.path(), "test"))
        .with_flow_lock_root(self.locks());
        RunsHost::install(opened, cx);
    }

    fn restart(&self, dirs: &Dirs, cx: &mut App) {
        RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
        self.start(dirs, cx);
    }
}

/// The orchestrator owns the flow at `location`, as a start of its own does.
fn own_flow(location: FlowRunLocation, cx: &mut App) -> RunKey {
    let key = RunKey::flow_of(&location.run_dir);
    cx.update_global::<RunsHost, _>(|host, _| host.watch.locate(location));
    RunsHost::register_run_solicited(key.clone(), true, cx);
    key
}

fn ended_as(event: &crate::runs::watch::RunEvent) -> Option<EndedAs> {
    match &event.snapshot.state {
        RunState::Ended { ended_as, .. } => Some(*ended_as),
        _ => None,
    }
}

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

fn wait_for_file(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.is_file() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
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

fn on_watch<R>(cx: &mut App, f: impl FnOnce(&mut crate::runs::watch::RunWatch) -> R) -> R {
    cx.update_global::<RunsHost, _>(|host, _| f(&mut host.watch))
}

fn end_of(key: &RunKey, ended_as: EndedAs, what: &str) -> RunSnapshot {
    RunSnapshot {
        key: key.clone(),
        what: what.into(),
        why: None,
        state: RunState::Ended {
            ended_as,
            text: None,
            pointers: Default::default(),
        },
    }
}

/// One wait over `keys` (the omitted scope when `None`), answered and receipted.
fn report(keys: Option<Vec<RunKey>>, cx: &mut App) -> crate::runs::watch::WaitAnswer {
    let (id, _) = RunsHost::open_run_wait(keys, cx).unwrap();
    let answer = RunsHost::poll_run_wait(id, false, cx).unwrap();
    RunsHost::finish_run_wait(id, true, cx);
    answer
}

/// Whether the ledger still writes once every queued record is answered.
fn writing_after_flush(cx: &mut App) -> bool {
    RunsHost::quit_flush(Instant::now() + Duration::from_secs(5), cx);
    RunsHost::run_ledger_writing_for_test(cx)
}

fn whats(answer: &crate::runs::watch::WaitAnswer) -> Vec<&str> {
    answer
        .ended
        .iter()
        .map(|end| end.snapshot.what.as_str())
        .collect()
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
