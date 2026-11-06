//! The ledger's single writer: fire-and-forget records in proposal order.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use daruda_runlog::{
    CommitError, Committed, Committer, FileBackend, FlushTimedOut, OpenError, RunLog,
};

use super::{LedgerRejected, RunLedgerEvent, RunLedgerMachine, RunLedgerState};

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
        let committer = Committer::spawn(log, "run-ledger", Box::new(|_, _| {}))?;
        Ok((
            Self {
                committer,
                answers: Mutex::default(),
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

    /// Wait until every earlier record is answered.
    pub fn flush(&self, deadline: Duration) -> Result<(), FlushTimedOut> {
        self.committer.flush(deadline)
    }

    fn answers(&self) -> std::sync::MutexGuard<'_, VecDeque<Answer>> {
        self.answers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
