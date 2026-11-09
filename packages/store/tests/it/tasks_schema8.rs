//! Task data written by schema 8, when a task still named a terminal surface
//! and runs could drive the Claude CLI, keeps opening. The fixtures are files
//! that build wrote: an event log, a compacted snapshot with no older
//! generation to fall back to, and a legacy `tasks.json`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use daruda_runlog::AfterCommit;
use daruda_store::tasks::machine::{
    InterruptCause, Lifecycle, LifecycleKind, TaskMachine, TasksBook,
};
use daruda_store::tasks::{
    CliProcessState, CliSessionEndReason, TaskExecution, TaskExecutionSource, TaskStore,
    tasks_path_in,
};

const BUILD: &str = "daruda_store-test";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/it/fixtures")
        .join(name)
}

fn no_hook() -> AfterCommit<TaskMachine> {
    Box::new(|_, _| {})
}

/// A writable copy of a fixture log directory: opening appends a record.
fn copied_log(name: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(fixture(name)).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
    }
    dir
}

fn book_of(store: &TaskStore) -> TasksBook {
    store.flush(Duration::from_secs(5)).unwrap();
    store.current().unwrap().1
}

fn cli_run(execution: &TaskExecution) -> (Option<&Path>, &CliProcessState) {
    match &execution.source {
        TaskExecutionSource::ClaudeCli {
            transcript_path,
            process,
        } => (transcript_path.as_deref(), process),
        other => panic!("not a CLI run: {other:?}"),
    }
}

/// What the fixture log's events build, read either from the events or from
/// the snapshot they were compacted into.
fn assert_schema_eight_book(book: &TasksBook) {
    assert_eq!(book.len(), 5);
    assert_eq!(book.get("01BACKLOG").unwrap().lifecycle, Lifecycle::Backlog);

    let running = book.get("01RUNNING").unwrap();
    assert_eq!(running.lifecycle.kind(), LifecycleKind::Running);
    assert_eq!(running.spec.title, "running edited");
    let execution = running.execution.as_ref().unwrap();
    assert_eq!(execution.terminal_id.as_deref(), Some("term-01RUNNING"));
    assert_eq!(execution.session_id.as_deref(), Some("s-running"));
    assert_eq!(
        cli_run(execution),
        (
            Some(Path::new("/home/u/.claude/projects/repo/s-running.jsonl")),
            &CliProcessState::Running { pid: 4242 }
        )
    );
    assert_eq!(running.session_ids, ["s-running"]);
    assert_eq!(running.subtasks.len(), 1);
    assert_eq!(running.subtasks[0].title, "step one");

    let ended = book.get("01ENDED").unwrap();
    assert_eq!(ended.lifecycle.kind(), LifecycleKind::Done);
    let [result] = ended.ended_executions.as_slice() else {
        panic!("one ended run: {:?}", ended.ended_executions);
    };
    assert_eq!(result.result.as_deref(), Some("Changed the login flow."));
    let execution = result.execution.as_ref().unwrap();
    assert_eq!(
        cli_run(execution).0,
        Some(Path::new("/home/u/.claude/projects/repo/s-ended.jsonl"))
    );
    assert_eq!(
        ended
            .execution
            .as_ref()
            .unwrap()
            .cli_session_end
            .as_ref()
            .map(|end| end.reason),
        Some(CliSessionEndReason::SessionEnded)
    );

    let preparing = book.get("01PREPARING").unwrap();
    let Lifecycle::Preparing { source, .. } = &preparing.lifecycle else {
        panic!("{:?}", preparing.lifecycle);
    };
    assert!(matches!(source, TaskExecutionSource::ClaudeCli { .. }));

    let dispatching = book.get("01DISPATCHING").unwrap();
    assert_eq!(dispatching.lifecycle.kind(), LifecycleKind::Dispatching);
    let execution = dispatching.execution.as_ref().unwrap();
    assert_eq!(execution.terminal_id.as_deref(), Some("term-01DISPATCHING"));
    assert_eq!(cli_run(execution).1, &CliProcessState::Discovering);
}

#[test]
fn a_schema_eight_event_log_still_replays() {
    let dir = copied_log("task_log_schema8");
    let store = TaskStore::in_directory(dir.path(), None, BUILD, no_hook()).unwrap();
    assert_schema_eight_book(&book_of(&store));
}

#[test]
fn a_schema_eight_snapshot_still_opens() {
    let dir = copied_log("task_snapshot_schema8");
    let store = TaskStore::in_directory(dir.path(), None, BUILD, no_hook()).unwrap();
    assert_schema_eight_book(&book_of(&store));
}

#[test]
fn a_tasks_file_naming_a_terminal_surface_still_imports() {
    let data = tempfile::tempdir().unwrap();
    std::fs::copy(fixture("tasks_schema8.json"), tasks_path_in(data.path())).unwrap();
    let log = tempfile::tempdir().unwrap();
    let store = TaskStore::in_directory(log.path(), Some(data.path()), BUILD, no_hook()).unwrap();
    let book = book_of(&store);

    assert_eq!(book.len(), 2);
    assert_eq!(
        book.get("01LEGACYBACKLOG").unwrap().lifecycle,
        Lifecycle::Backlog
    );
    let running = book.get("01LEGACYRUN").unwrap();
    assert!(matches!(
        running.lifecycle,
        Lifecycle::Interrupted {
            cause: InterruptCause::AppRestart,
            ..
        }
    ));
    let execution = running.execution.as_ref().unwrap();
    assert_eq!(execution.terminal_id.as_deref(), Some("term-legacy"));
    assert_eq!(
        cli_run(execution),
        (
            Some(Path::new("/home/u/.claude/projects/repo/s-legacy.jsonl")),
            &CliProcessState::Running { pid: 77 }
        )
    );
}

/// Specs and edits stopped carrying a surface and an auto-execute flag.
#[test]
fn task_specs_without_a_surface_are_schema_nine() {
    use daruda_runlog::Machine as _;
    assert_eq!(TaskMachine::SCHEMA, 9);
}

/// Once this build has written to a schema 8 log, a schema 8 build refuses
/// it as too new rather than as corrupt.
#[test]
fn a_schema_eight_build_refuses_the_log_once_this_build_wrote_it() {
    use daruda_runlog::{FileBackend, Machine, OpenError, RunLog, Versioned};
    use daruda_store::tasks::machine::{TaskEvent, TaskRejected};

    struct SchemaEight;
    impl Machine for SchemaEight {
        type State = TasksBook;
        type Event = TaskEvent;
        type Rejected = TaskRejected;
        const SCHEMA: u32 = 8;
        fn check(state: &TasksBook, event: &TaskEvent) -> Result<(), TaskRejected> {
            TaskMachine::check(state, event)
        }
        fn apply(state: &mut TasksBook, event: &TaskEvent) {
            TaskMachine::apply(state, event);
        }
    }

    let dir = copied_log("task_log_schema8");
    drop(TaskStore::in_directory(dir.path(), None, BUILD, no_hook()).unwrap());
    assert!(matches!(
        RunLog::<SchemaEight, _>::open(FileBackend::new(dir.path()), "old"),
        Err(OpenError::TooNew {
            what: Versioned::Schema,
            found: 9,
            supported: 8
        })
    ));
}
