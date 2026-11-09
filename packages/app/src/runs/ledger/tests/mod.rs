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

mod flows;
mod restart;
mod resume;
mod retention;

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
