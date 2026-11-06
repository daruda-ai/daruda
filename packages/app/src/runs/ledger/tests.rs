//! Restart scenarios: a host is dropped and reopened from the same directories.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use daruda_store::run_ledger::RunLedgerStore;
use daruda_store::tasks::machine::StartedBy;
use daruda_store::tasks::{TaskEvent, TaskStore};
use gpui::{App, BorrowAppContext as _, TestAppContext};

use super::*;
use crate::runs::seed::{self, SeedEnd, SeedRun};
use crate::runs::watch::{Origin, WaitOutcome};
use crate::runs::{GlobalTasks, RunsHost, no_hook};

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
        assert_eq!(answer.recovered, vec![key.clone()]);
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
        assert_eq!(pending[0].origin, Origin::Live);
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
    let text =
        "version: 1\nname: Restarted\nnodes:\n  - id: ok\n    kind: command\n    run: \"exit 0\"\n";
    let run_dir = crate::runs::runs_dir(lane).join(run_id);
    let request = daruda_flow::request::RunRequest {
        history: Default::default(),
        started_by: Some(daruda_flow::request::StartedBy::Orchestrator),
        loaded: daruda_flow::load(text, None).unwrap(),
        cwd: lane.to_path_buf(),
        run_dir: run_dir.clone(),
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
    };
    let runners = daruda_flow::runner::Runners {
        command: daruda_flow::runner::ProcessRunner::new(Vec::new()),
        agent: daruda_flow::runner::AcpRunner::new(Default::default(), lane.join("node")),
    };
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

#[gpui::test]
async fn a_task_whose_flow_was_interrupted_reads_resumable_then_settles_after_a_resume(
    cx: &mut TestAppContext,
) {
    use daruda_store::tasks::{LifecycleKind, TaskExecutionSource};
    let (dirs, lane) = (Dirs::new(), Lane::new());
    let run_id = "task-flow";
    let run = SeedRun {
        source: TaskExecutionSource::Flow {
            run_id: run_id.into(),
            flow: "restarted.yaml".into(),
        },
        ..SeedRun::chat(lane.path())
    };
    let (id, key) = cx.update(|cx| {
        lane.start(&dirs, cx);
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
    cx.update(|cx| lane.restart(&dirs, cx));
    cx.run_until_parked();
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

    // A person resumes it from the Flows panel; it ends, and so does the task.
    cx.update(|cx| {
        RunsHost::seed_flow_run(
            lane.path(),
            crate::runs::RunView::new(
                run_dir.clone(),
                crate::runs::FlowSource::Resumed {
                    run_dir: run_dir.clone(),
                },
                Vec::new(),
            ),
            cx,
        );
        RunsHost::flow_task_resumed(&run_dir, cx);
        assert!(matches!(
            RunsHost::run_query(key.clone(), cx).read().state,
            RunState::Running { .. }
        ));
        RunsHost::flow_task_ended(
            &run_dir,
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
    assert_eq!(lifecycle(cx), LifecycleKind::Done);
    cx.update(|cx| {
        let pending = RunsHost::unreported_run_events(cx);
        assert_eq!(pending.len(), 1, "{pending:?}");
        assert_eq!(pending[0].snapshot.key, key);
        assert_eq!(ended_as(&pending[0]), Some(EndedAs::Completed));
        assert_eq!(pending[0].origin, Origin::Live);
    });
}
