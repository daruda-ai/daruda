//! The ledger's single writer: fire-and-forget records in proposal order.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use daruda_runlog::{
    Appended, CommitError, Committer, Compaction, FileBackend, FlushTimedOut, OpenError, RunLog,
};

use super::{
    DirectoryHolds, LedgerKey, LedgerRejected, RunLedgerEvent, RunLedgerMachine, RunLedgerState,
};

type Answer = Pin<
    Box<dyn Future<Output = Result<Appended<RunLedgerEvent>, CommitError<LedgerRejected>>> + Send>,
>;

#[derive(Debug, thiserror::Error)]
pub enum RunLedgerError {
    #[error("run ledger directory could not be prepared: {0}")]
    Io(#[from] io::Error),
    #[error("run ledger could not be opened: {0}")]
    Open(#[from] OpenError),
}

/// What the records answered since the last look tell their writer.
#[derive(Debug, Default)]
pub struct Answered {
    /// The first record the machine refused. The log stays open after one.
    pub rejected: Option<LedgerRejected>,
    /// The first failure that stopped the log: every later record fails too.
    pub failure: Option<CommitError<LedgerRejected>>,
    /// The key of each end the committed ends evicted to stay under the cap,
    /// in commit order.
    pub evicted: Vec<LedgerKey>,
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
        Self::compacting(dir, build, Compaction::WhenDue)
    }

    /// [`Self::in_directory`], starting a new generation as `compaction`
    /// says; nothing else ever compacts the ledger.
    pub(crate) fn compacting(
        dir: impl Into<PathBuf>,
        build: &str,
        compaction: Compaction,
    ) -> Result<(Self, RunLedgerState), RunLedgerError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let log = RunLog::<RunLedgerMachine, _>::open(FileBackend::new(dir), build)?;
        let state = log.state().clone();
        let holds = Arc::new(Mutex::new(state.directory_holds()));
        let projection = Arc::clone(&holds);
        let committer = Committer::spawn_with(
            log,
            "run-ledger",
            Box::new(move |_, state: &RunLedgerState| {
                *lock(&projection) = state.directory_holds();
            }),
            compaction,
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

    /// Queue `event` without waiting. Its outcome surfaces later through
    /// [`Self::take_answered`]; after an I/O failure every later record fails.
    pub fn record(&self, event: RunLedgerEvent) {
        let answer = Box::pin(self.committer.append(event));
        self.answers().push_back(answer);
    }

    /// What the records answered so far report, each once. Records still
    /// being written are looked at on a later call.
    pub fn take_answered(&self) -> Answered {
        let mut answers = self.answers();
        let mut answered = Answered::default();
        while let Some(answer) = answers.front_mut() {
            let Some(result) = smol::future::block_on(smol::future::poll_once(answer)) else {
                break;
            };
            answers.pop_front();
            match result {
                Ok(appended) => {
                    if let RunLedgerEvent::Ended { evicts, .. } = appended.event {
                        answered.evicted.extend(evicts);
                    }
                }
                Err(CommitError::Rejected(rejected)) => {
                    answered.rejected.get_or_insert(rejected);
                }
                Err(error) => {
                    answered.failure.get_or_insert(error);
                }
            }
        }
        answered
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
