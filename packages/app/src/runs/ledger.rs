//! Durable orchestrator delivery: what `RunWatch` must not forget across a
//! restart, written best-effort beside its in-memory ledger.
//!
//! Writes are queued without waiting, in the order the watch decides them, so
//! delivery timing never depends on the disk. A failed open or write leaves
//! delivery memory-only for the rest of the process, as before the ledger.

use std::time::Instant;

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::run_ledger::{
    LedgerEnd, LedgerEndedAs, LedgerKey, LedgerPointers, ReleaseReason, RunLedgerEntry,
    RunLedgerError, RunLedgerEvent, RunLedgerState, RunLedgerStore, RunLocation,
};
use daruda_store::tasks::{ExecutionRef, TasksBook};

use super::result::{EndedAs, FlowRunLocation, Pointers, RunKey, RunSnapshot, RunState};

/// What opening the profile's ledger produced.
pub(crate) type OpenedLedger = Result<(RunLedgerStore, RunLedgerState), RunLedgerError>;

#[derive(Default)]
pub(crate) enum Ledger {
    /// No ledger in this process: delivery is memory-only.
    #[default]
    Off,
    Writing {
        store: RunLedgerStore,
        /// The proposer numbers ends so it can tag its result before the commit.
        next_episode: u64,
    },
    /// A write failed; later ones would leave a gap, so none is attempted.
    Stopped,
}

/// A ledger entry as the watch takes it back after a restart.
pub(crate) struct Restored {
    pub(crate) key: RunKey,
    pub(crate) solicited: bool,
    pub(crate) location: Option<FlowRunLocation>,
    pub(crate) end: RestoredEnd,
}

pub(crate) enum RestoredEnd {
    /// Owned, with no end recorded yet.
    Open,
    Unreported {
        episode: u64,
        snapshot: RunSnapshot,
    },
    /// Its latest end was handed to an agent; kept only to stay in scope.
    Reported(RunSnapshot),
}

impl Ledger {
    /// The opened ledger and the entries it replayed, or no ledger and one
    /// warning when it could not be opened. Nothing on disk is changed.
    pub(crate) fn open(opened: OpenedLedger) -> (Self, Vec<RunLedgerEntry>) {
        match opened {
            Ok((store, state)) => (
                Self::Writing {
                    store,
                    next_episode: state.next_episode(),
                },
                state.entries().to_vec(),
            ),
            Err(error) => {
                LogWriter::log(open_failure(&error));
                (Self::Off, Vec::new())
            }
        }
    }

    /// Turn replayed entries into what the watch restores. A task execution has
    /// one end, so one already reported, or one whose task is gone with nothing
    /// to deliver, is released instead. `book` is `None` when the task log
    /// could not be read: no task entry is judged then.
    pub(crate) fn restore(
        &mut self,
        entries: Vec<RunLedgerEntry>,
        book: Option<&TasksBook>,
    ) -> Vec<Restored> {
        let mut restored = Vec::new();
        for entry in entries {
            let key = key_of(&entry.key);
            let end = match (entry.unreported(), &entry.ended) {
                (Some(ended), _) => RestoredEnd::Unreported {
                    episode: ended.episode,
                    snapshot: snapshot_of(key.clone(), &ended.end),
                },
                (None, Some(_)) if matches!(key, RunKey::Task(_)) => {
                    self.release(&entry.key, ReleaseReason::Reported);
                    continue;
                }
                (None, Some(ended)) => RestoredEnd::Reported(snapshot_of(key.clone(), &ended.end)),
                (None, None) => match (&key, book) {
                    (RunKey::Task(_), None) => continue,
                    (RunKey::Task(exec), Some(book)) if !holds(book, exec) => {
                        self.release(&entry.key, ReleaseReason::Expired);
                        continue;
                    }
                    _ => RestoredEnd::Open,
                },
            };
            restored.push(Restored {
                key,
                solicited: entry.solicited,
                location: entry.location.as_ref().map(location_of),
                end,
            });
        }
        restored
    }

    pub(super) fn owned(
        &mut self,
        key: &RunKey,
        solicited: bool,
        location: Option<&FlowRunLocation>,
    ) {
        self.record(RunLedgerEvent::Owned {
            key: ledger_key(key),
            solicited,
            location: location.map(|location| RunLocation {
                run_dir: location.run_dir.clone(),
                lock_dir: location.lock_dir.clone(),
            }),
        });
    }

    /// Record a new end and answer the episode a later report names; `None`
    /// when nothing is written or `snapshot` is not an end.
    pub(super) fn ended(&mut self, snapshot: &RunSnapshot, solicited: bool) -> Option<u64> {
        let end = ledger_end(snapshot)?;
        let Self::Writing { next_episode, .. } = self else {
            return None;
        };
        let episode = *next_episode;
        *next_episode += 1;
        self.record(RunLedgerEvent::Ended {
            key: ledger_key(&snapshot.key),
            solicited,
            episode,
            end,
            evicts: Vec::new(),
        });
        Some(episode)
    }

    /// Replace a recorded end's content, after its text was read.
    pub(super) fn rerecord(&mut self, episode: u64, snapshot: &RunSnapshot, solicited: bool) {
        if let Some(end) = ledger_end(snapshot) {
            self.record(RunLedgerEvent::Ended {
                key: ledger_key(&snapshot.key),
                solicited,
                episode,
                end,
                evicts: Vec::new(),
            });
        }
    }

    /// An agent received `episode`. When that is the key's current end the
    /// entry has nothing left to deliver and is released with it; an older
    /// episode keeps the entry for the run's newer one.
    pub(super) fn reported(&mut self, key: &RunKey, episode: u64, current: bool) {
        let key = ledger_key(key);
        self.record(RunLedgerEvent::Reported {
            key: key.clone(),
            episode,
        });
        if current {
            self.release(&key, ReleaseReason::Reported);
        }
    }

    /// Make every queued write durable by `until`, as the app quits.
    pub(crate) fn flush(&mut self, until: Instant) {
        let Self::Writing { store, .. } = self else {
            return;
        };
        if let Err(error) = store.flush(until.saturating_duration_since(Instant::now())) {
            LogWriter::log(
                ErrorReport::new("Run ledger writes were still queued at quit")
                    .severity(ErrorSeverity::Warning)
                    .from_error(&error)
                    .at(file!(), line!())
                    .dedup("run_ledger.quit_flush")
                    .build(),
            );
        }
        self.check();
    }

    fn release(&mut self, key: &LedgerKey, reason: ReleaseReason) {
        self.record(RunLedgerEvent::Released {
            key: key.clone(),
            reason,
        });
    }

    fn record(&mut self, event: RunLedgerEvent) {
        self.check();
        if let Self::Writing { store, .. } = self {
            store.record(event);
        }
    }

    /// Stop at the first failed write, warning once.
    fn check(&mut self) {
        let Self::Writing { store, .. } = self else {
            return;
        };
        let Some(error) = store.take_failure() else {
            return;
        };
        LogWriter::log(
            ErrorReport::new("Run results are no longer kept across restarts")
                .severity(ErrorSeverity::Warning)
                .from_error(&error)
                .at(file!(), line!())
                .dedup("run_ledger.commit")
                .build(),
        );
        *self = Self::Stopped;
    }
}

fn open_failure(error: &RunLedgerError) -> ErrorReport {
    ErrorReport::new("Run results will not be kept across restarts")
        .severity(ErrorSeverity::Warning)
        .from_error(error)
        .at(file!(), line!())
        .dedup("run_ledger.open")
        .build()
}

fn holds(book: &TasksBook, exec: &ExecutionRef) -> bool {
    book.get(&exec.task_id).is_some_and(|record| {
        record.current_exec() == Some(exec)
            || record.ended_executions.iter().any(|end| &end.exec == exec)
    })
}

fn ledger_key(key: &RunKey) -> LedgerKey {
    match key {
        RunKey::Flow(run_id) => LedgerKey::Flow {
            run_id: run_id.clone(),
        },
        RunKey::Task(exec) => LedgerKey::Task(exec.clone()),
    }
}

fn key_of(key: &LedgerKey) -> RunKey {
    match key {
        LedgerKey::Flow { run_id } => RunKey::Flow(run_id.clone()),
        LedgerKey::Task(exec) => RunKey::Task(exec.clone()),
    }
}

fn location_of(location: &RunLocation) -> FlowRunLocation {
    FlowRunLocation {
        run_dir: location.run_dir.clone(),
        lock_dir: location.lock_dir.clone(),
    }
}

fn ledger_end(snapshot: &RunSnapshot) -> Option<LedgerEnd> {
    let RunState::Ended {
        ended_as,
        text,
        pointers,
    } = &snapshot.state
    else {
        return None;
    };
    Some(LedgerEnd {
        what: snapshot.what.clone(),
        why: snapshot.why.clone(),
        ended_as: match ended_as {
            EndedAs::Completed => LedgerEndedAs::Completed,
            EndedAs::Failed => LedgerEndedAs::Failed,
            EndedAs::Canceled => LedgerEndedAs::Canceled,
            EndedAs::Stalled => LedgerEndedAs::Stalled,
            EndedAs::Crashed => LedgerEndedAs::Crashed,
            EndedAs::Interrupted => LedgerEndedAs::Interrupted,
            EndedAs::Undelivered => LedgerEndedAs::Undelivered,
            EndedAs::Error => LedgerEndedAs::Error,
            EndedAs::BudgetExhausted => LedgerEndedAs::BudgetExhausted,
        },
        text: text.clone(),
        pointers: LedgerPointers {
            run_dir: pointers.run_dir.clone(),
            report: pointers.report.clone(),
            outputs: pointers.outputs.clone(),
            chat: pointers.chat.clone(),
            transcript: pointers.transcript.clone(),
        },
    })
}

fn snapshot_of(key: RunKey, end: &LedgerEnd) -> RunSnapshot {
    RunSnapshot {
        key,
        what: end.what.clone(),
        why: end.why.clone(),
        state: RunState::Ended {
            ended_as: match end.ended_as {
                LedgerEndedAs::Completed => EndedAs::Completed,
                LedgerEndedAs::Failed => EndedAs::Failed,
                LedgerEndedAs::Canceled => EndedAs::Canceled,
                LedgerEndedAs::Stalled => EndedAs::Stalled,
                LedgerEndedAs::Crashed => EndedAs::Crashed,
                LedgerEndedAs::Interrupted => EndedAs::Interrupted,
                LedgerEndedAs::Undelivered => EndedAs::Undelivered,
                LedgerEndedAs::Error => EndedAs::Error,
                LedgerEndedAs::BudgetExhausted => EndedAs::BudgetExhausted,
            },
            text: end.text.clone(),
            pointers: Pointers {
                run_dir: end.pointers.run_dir.clone(),
                report: end.pointers.report.clone(),
                outputs: end.pointers.outputs.clone(),
                chat: end.pointers.chat.clone(),
                transcript: end.pointers.transcript.clone(),
            },
        },
    }
}

#[cfg(test)]
impl super::RunsHost {
    pub(crate) fn run_ledger_writing_for_test(cx: &gpui::App) -> bool {
        matches!(cx.global::<Self>().watch.ledger, Ledger::Writing { .. })
    }

    /// Wait until every queued ledger record is answered, without quitting.
    pub(crate) fn flush_run_ledger_for_test(cx: &gpui::App) {
        if let Ledger::Writing { store, .. } = &cx.global::<Self>().watch.ledger {
            store.flush(std::time::Duration::from_secs(5)).unwrap();
        }
    }
}

#[cfg(test)]
mod tests;
