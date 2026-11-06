//! The ledger's single writer: fire-and-forget records in proposal order.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use daruda_runlog::{
    Appended, CommitError, Committer, Compaction, FileBackend, FlushTimedOut, OpenError, RunLog,
};

use super::{
    DirectoryHolds, LedgerKey, LedgerRejected, RunLedgerEvent, RunLedgerMachine, RunLedgerState,
};

/// Told, on the ledger's writer thread in commit order, each flow run
/// directory whose dated hold changed: `Some(until_ms)` once it is held until
/// then, `None` once it is no longer held with a deadline.
pub type MarkHold = Box<dyn Fn(&Path, Option<u64>) + Send>;

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
    /// Open the profile's ledger and the state it replays to, telling `mark`
    /// how its holds change. The caller must hold the profile's desktop
    /// instance lock: the log assumes one writer.
    pub fn open_current(
        build: &str,
        mark: MarkHold,
    ) -> Result<(Self, RunLedgerState), RunLedgerError> {
        Self::opening(
            crate::storage::StorageLayout::current().run_ledger(),
            build,
            Compaction::WhenDue,
            Some(mark),
        )
    }

    /// Open the ledger in `dir`. An unreadable or newer log is refused and
    /// left byte-for-byte as it was; the caller runs without one.
    pub fn in_directory(
        dir: impl Into<PathBuf>,
        build: &str,
    ) -> Result<(Self, RunLedgerState), RunLedgerError> {
        Self::opening(dir, build, Compaction::WhenDue, None)
    }

    /// [`Self::in_directory`], telling `mark` how its holds change.
    pub fn marking(
        dir: impl Into<PathBuf>,
        build: &str,
        mark: MarkHold,
    ) -> Result<(Self, RunLedgerState), RunLedgerError> {
        Self::opening(dir, build, Compaction::WhenDue, Some(mark))
    }

    /// [`Self::in_directory`], starting a new generation as `compaction`
    /// says; nothing else ever compacts the ledger.
    #[cfg(test)]
    pub(crate) fn compacting(
        dir: impl Into<PathBuf>,
        build: &str,
        compaction: Compaction,
    ) -> Result<(Self, RunLedgerState), RunLedgerError> {
        Self::opening(dir, build, compaction, None)
    }

    fn opening(
        dir: impl Into<PathBuf>,
        build: &str,
        compaction: Compaction,
        mark: Option<MarkHold>,
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
                let next = state.directory_holds();
                let changes = {
                    let mut held = lock(&projection);
                    let changes = mark.as_ref().map(|_| held.marker_changes(&next));
                    *held = next;
                    changes
                };
                // Outside the lock: a reader of the holds never waits on disk.
                if let (Some(mark), Some(changes)) = (&mark, changes) {
                    for (run_dir, until) in changes {
                        mark(&run_dir, until);
                    }
                }
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

    /// A handle another thread can wait on until every record proposed
    /// before its wait began is answered.
    pub fn barrier(&self) -> LedgerBarrier {
        LedgerBarrier(self.committer.clone())
    }

    fn answers(&self) -> MutexGuard<'_, VecDeque<Answer>> {
        lock(&self.answers)
    }
}

/// See [`RunLedgerStore::barrier`]. It keeps the ledger's writer alive
/// until it is waited on or dropped.
pub struct LedgerBarrier(Committer<RunLedgerMachine>);

impl LedgerBarrier {
    /// Block until every record proposed before this call is answered, or
    /// `deadline` passes.
    pub fn wait(self, deadline: Duration) -> Result<(), FlushTimedOut> {
        self.0.flush(deadline)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
