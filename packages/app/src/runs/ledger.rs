//! Durable orchestrator delivery: what `RunWatch` must not forget across a
//! restart, written best-effort beside its in-memory ledger.
//!
//! Writes are queued without waiting, in the order the watch decides them, so
//! delivery timing never depends on the disk. A failed open or write leaves
//! delivery memory-only for the rest of the process, as before the ledger; a
//! record the ledger refuses is logged and the ones after it still land.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use daruda_store::observability::log_writer::LogWriter;
use daruda_store::run_ledger::{
    Answered, DIRECTORY_HOLD, LedgerBarrier, LedgerKey, LedgerRejected, MarkHold, ReleaseReason,
    RunLedgerEntry, RunLedgerError, RunLedgerEvent, RunLedgerState, RunLedgerStore,
};

use super::result::{FlowRunLocation, RunKey, RunSnapshot};
use convert::{ledger_end, ledger_key, ledger_key_of, run_location};

mod convert;
mod restore;

pub(crate) use restore::{OwedEnd, Restored, RestoredEnd};

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
        /// Restored flows open for longer than the directory hold: one that
        /// still cannot be judged is let go instead of kept forever.
        lapsed: HashSet<LedgerKey>,
    },
    /// A write failed; later ones would leave a gap, so none is attempted.
    /// The store still answers the holds its last committed record left.
    Stopped { store: RunLedgerStore },
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
                    lapsed: HashSet::new(),
                },
                state.entries().to_vec(),
            ),
            Err(error) => {
                LogWriter::log(open_failure(&error));
                (Self::Off, Vec::new())
            }
        }
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
            location: location.map(run_location),
            at_ms: Some(now_ms()),
        });
    }

    /// An owned run started again; the ledger keeps an unreported end owed.
    pub(super) fn reopened(
        &mut self,
        key: &RunKey,
        solicited: bool,
        location: Option<&FlowRunLocation>,
    ) {
        self.record(RunLedgerEvent::Reopened {
            key: ledger_key(key),
            solicited,
            location: location.map(run_location),
            at_ms: Some(now_ms()),
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
            ended_at_ms: Some(now_ms()),
        });
        Some(episode)
    }

    /// Replace a recorded end's content, after its text was read. Its date
    /// is not the reader's to give: the end keeps the one it has.
    pub(super) fn rerecord(&mut self, episode: u64, snapshot: &RunSnapshot, solicited: bool) {
        if let Some(end) = ledger_end(snapshot) {
            self.record(RunLedgerEvent::Ended {
                key: ledger_key(&snapshot.key),
                solicited,
                episode,
                end,
                evicts: Vec::new(),
                ended_at_ms: None,
            });
        }
    }

    /// An agent received the recorded end numbered `episode`.
    pub(super) fn reported(&mut self, key: &RunKey, episode: u64) {
        self.record(RunLedgerEvent::Reported {
            key: ledger_key(key),
            episode,
        });
    }

    /// Every recorded end of the run was handed to an agent and its run is
    /// over: nothing is left to deliver.
    pub(super) fn settled(&mut self, key: &RunKey) {
        self.release(&ledger_key(key), ReleaseReason::Reported);
    }

    /// The run is gone with nothing delivered: no later process can either.
    pub(super) fn expired(&mut self, key: &RunKey) {
        self.release(&ledger_key(key), ReleaseReason::Expired);
    }

    /// A restored flow that still cannot be judged after the directory hold
    /// is let go; its directory, if any, is left to retention. Whether it was.
    pub(super) fn release_if_lapsed(&mut self, key: &RunKey) -> bool {
        let key = ledger_key(key);
        let Self::Writing { lapsed, .. } = self else {
            return false;
        };
        let released = lapsed.remove(&key);
        if released {
            LogWriter::log(lapse());
            self.release(&key, ReleaseReason::Expired);
        }
        released
    }

    /// Flow runs, by id, whose directories the ledger holds at `now_ms`, as of
    /// its committed records. Nothing is held without a ledger.
    pub(crate) fn held_run_dirs(&self, now_ms: u64) -> HashSet<String> {
        match self {
            Self::Writing { store, .. } | Self::Stopped { store } => store.held_run_dirs(now_ms),
            Self::Off => HashSet::new(),
        }
    }

    /// What a run about to start waits on, so that every record proposed
    /// before it starts lands before anything the run writes. `None` without
    /// a ledger still writing.
    pub(crate) fn barrier(&self) -> Option<LedgerBarrier> {
        match self {
            Self::Writing { store, .. } => Some(store.barrier()),
            Self::Off | Self::Stopped { .. } => None,
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
        if let Self::Writing { store, lapsed, .. } = self {
            // Anything the run does again makes it judgeable, or released.
            lapsed.remove(ledger_key_of(&event));
            store.record(event);
        }
    }

    fn check(&mut self) {
        if let Self::Writing { store, .. } = self {
            let answered = store.take_answered();
            self.observe(answered);
        }
    }

    /// Note the ends the cap made the ledger forget and a refused record,
    /// and stop at the first failed write, warning once. A refusal leaves
    /// the log open: the records after it still land.
    pub(super) fn observe(&mut self, answered: Answered) {
        if !answered.evicted.is_empty() {
            LogWriter::log(eviction(&answered.evicted));
        }
        if let Some(rejected) = &answered.rejected {
            LogWriter::log(rejection(rejected));
        }
        let Some(error) = answered.failure else {
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
        *self = match std::mem::take(self) {
            Self::Writing { store, .. } => Self::Stopped { store },
            other => other,
        };
    }
}

/// Mirror each dated directory hold the ledger commits into a marker in the
/// run's own directory, so another profile's retention spares it too. A run
/// already gone has nothing to hold; any other failure is logged once.
pub(crate) fn hold_markers() -> MarkHold {
    Box::new(|run_dir: &Path, until: Option<u64>| {
        let written = match until {
            Some(until) => daruda_flow::marker::hold(run_dir, until),
            None => daruda_flow::marker::release_hold(run_dir),
        };
        match written {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => LogWriter::log(
                ErrorReport::new("A flow run directory's hold marker could not be updated")
                    .severity(ErrorSeverity::Warning)
                    .from_error(&error)
                    .at(file!(), line!())
                    .dedup("run_ledger.hold_marker")
                    .build(),
            ),
            _ => {}
        }
    })
}

/// A refused record is the proposer's bug: it never stops the ledger.
fn rejection(rejected: &LedgerRejected) -> ErrorReport {
    ErrorReport::new("The run ledger refused a record")
        .severity(ErrorSeverity::Error)
        .with_context("refused", rejection_kind(rejected))
        .at(file!(), line!())
        .dedup("run_ledger.rejected")
        .build()
}

fn rejection_kind(rejected: &LedgerRejected) -> &'static str {
    match rejected {
        LedgerRejected::Unassigned { .. } => "unassigned_episode",
        LedgerRejected::InvalidEviction => "invalid_eviction",
    }
}

/// An owned flow no read could judge for a whole directory hold.
fn lapse() -> ErrorReport {
    ErrorReport::new("An owned flow run that could not be read left the run ledger")
        .severity(ErrorSeverity::Warning)
        .with_context(
            "open_for_days",
            (DIRECTORY_HOLD.as_secs() / 86_400).to_string(),
        )
        .at(file!(), line!())
        .dedup("run_ledger.unjudgeable")
        .build()
}

/// The clock the ledger dates ends with and judges directory holds against,
/// in Unix milliseconds; a clock before 1970 reads as 0.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn open_failure(error: &RunLedgerError) -> ErrorReport {
    ErrorReport::new("Run results will not be kept across restarts")
        .severity(ErrorSeverity::Warning)
        .from_error(error)
        .at(file!(), line!())
        .dedup("run_ledger.open")
        .build()
}

/// Unreported ends dropped past the cap are results no restart can deliver.
fn eviction(evicted: &[LedgerKey]) -> ErrorReport {
    let flows = evicted
        .iter()
        .filter(|key| matches!(key, LedgerKey::Flow { .. }))
        .count();
    ErrorReport::new("Older undelivered run results were dropped from the run ledger")
        .severity(ErrorSeverity::Warning)
        .with_context("evicted", evicted.len().to_string())
        .with_context("flows", flows.to_string())
        .with_context("tasks", (evicted.len() - flows).to_string())
        .at(file!(), line!())
        .dedup("run_ledger.evicted")
        .build()
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
