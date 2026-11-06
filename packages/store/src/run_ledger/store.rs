//! The ledger's single writer: fire-and-forget records in proposal order.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use daruda_runlog::{
    CommitError, Committed, Committer, FileBackend, FlushTimedOut, OpenError, RunLog,
};

use super::{DirectoryHolds, LedgerRejected, RunLedgerEvent, RunLedgerMachine, RunLedgerState};

type Answer = Pin<
    Box<
        dyn Future<Output = Result<Committed<RunLedgerMachine>, CommitError<LedgerRejected>>>
            + Send,
    >,
>;

#[derive(Debug, thiserror::Error)]
pub enum RunLedgerError {
    #[error("run ledger directory could not be prepared: {0}")]
    Io(#[from] io::Error),
    #[error("run ledger could not be opened: {0}")]
    Open(#[from] OpenError),
}

/// Domain code holds this, never the ledger's directory.
pub struct RunLedgerStore {
    committer: Committer<RunLedgerMachine>,
    /// Answers not yet looked at, oldest first; the committer answers in order.
    answers: Mutex<VecDeque<Answer>>,
    /// The committed state's holds, refreshed by the committer after each record.
    holds: Arc<Mutex<DirectoryHolds>>,
}

impl std::fmt::Debug for RunLedgerStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunLedgerStore")
            .field("committer", &self.committer)
            .finish_non_exhaustive()
    }
}

impl RunLedgerStore {
    /// Open the profile's ledger and the state it replays to. The caller must
    /// hold the profile's desktop instance lock: the log assumes one writer.
    pub fn open_current(build: &str) -> Result<(Self, RunLedgerState), RunLedgerError> {
        Self::in_directory(crate::storage::StorageLayout::current().run_ledger(), build)
    }

    /// Open the ledger in `dir`. An unreadable or newer log is refused and
    /// left byte-for-byte as it was; the caller runs without one.
    pub fn in_directory(
        dir: impl Into<PathBuf>,
        build: &str,
    ) -> Result<(Self, RunLedgerState), RunLedgerError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let log = RunLog::<RunLedgerMachine, _>::open(FileBackend::new(dir), build)?;
        let state = log.state().clone();
        let holds = Arc::new(Mutex::new(state.directory_holds()));
        let projection = Arc::clone(&holds);
        let committer = Committer::spawn(
            log,
            "run-ledger",
            Box::new(move |_, state: &RunLedgerState| {
                *lock(&projection) = state.directory_holds();
            }),
        )?;
        Ok((
            Self {
                committer,
                answers: Mutex::default(),
                holds,
            },
            state,
        ))
    }

    /// Queue `event` without waiting. A failure surfaces later through
    /// [`Self::take_failure`]; after an I/O failure every later record fails.
    pub fn record(&self, event: RunLedgerEvent) {
        let answer = Box::pin(self.committer.propose(event));
        self.answers().push_back(answer);
    }

    /// The first failure among the records answered so far, once. Records
    /// still being written are looked at on a later call.
    pub fn take_failure(&self) -> Option<CommitError<LedgerRejected>> {
        let mut answers = self.answers();
        let mut failure = None;
        while let Some(answer) = answers.front_mut() {
            let Some(result) = smol::future::block_on(smol::future::poll_once(answer)) else {
                break;
            };
            answers.pop_front();
            if let Err(error) = result
                && failure.is_none()
            {
                failure = Some(error);
            }
        }
        failure
    }

    /// Flow runs, by id, whose directories retention must spare at `now_ms`,
    /// as of the records committed so far: one still being written is not seen.
    pub fn held_run_dirs(&self, now_ms: u64) -> std::collections::HashSet<String> {
        lock(&self.holds).at(now_ms)
    }

    /// Wait until every earlier record is answered.
    pub fn flush(&self, deadline: Duration) -> Result<(), FlushTimedOut> {
        self.committer.flush(deadline)
    }

    fn answers(&self) -> MutexGuard<'_, VecDeque<Answer>> {
        lock(&self.answers)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
