//! What a new process takes back from the ledger, and which of its entries
//! the ledger lets go instead.

use daruda_store::run_ledger::{
    DIRECTORY_HOLD, EntryState, LedgerKey, ReleaseReason, RunLedgerEntry, RunLedgerEvent,
};
use daruda_store::tasks::{ExecutionRef, TasksBook};

use super::convert::{key_of, location_of, snapshot_of};
use super::{Ledger, now_ms};
use crate::runs::result::{FlowRunLocation, RunKey, RunSnapshot, RunState, is_interruption};

/// A ledger entry as the watch takes it back after a restart.
pub(crate) struct Restored {
    pub(crate) key: RunKey,
    pub(crate) solicited: bool,
    pub(crate) location: Option<FlowRunLocation>,
    /// Ends from before the run resumed, still unreported, oldest first.
    pub(crate) owed: Vec<OwedEnd>,
    pub(crate) end: RestoredEnd,
}

pub(crate) struct OwedEnd {
    pub(crate) episode: u64,
    pub(crate) snapshot: RunSnapshot,
}

impl OwedEnd {
    /// What stands for the run itself when only `state` is known of it,
    /// named as its latest owed end names it.
    fn stand_in(&self, state: RunState) -> RunSnapshot {
        RunSnapshot {
            key: self.snapshot.key.clone(),
            what: self.snapshot.what.clone(),
            why: None,
            state,
        }
    }
}

pub(crate) enum RestoredEnd {
    /// Owned or resumed, with no end of this run recorded yet.
    Open,
    Unreported {
        episode: u64,
        snapshot: RunSnapshot,
    },
    /// Its latest end was handed to an agent; kept only to stay in scope:
    /// an end still owed, a Task's flow interruption, or any Task the log
    /// cannot judge.
    Reported(RunSnapshot),
    /// Open, but no read in this process can judge it (an unreadable task
    /// log): it stays owned for a later one, and only what it owes is
    /// delivered. The snapshot says its state is unknown.
    Unjudged(RunSnapshot),
    /// A Task the log no longer holds, with ends it still owes: nothing more
    /// can end, so it is let go once those are reported. The snapshot says
    /// it is gone.
    Gone(RunSnapshot),
}

impl Ledger {
    /// Turn replayed entries into what the watch restores. A reported end
    /// owing nothing means its release never landed: released now, unless a
    /// Task's flow interruption the log holds (its own end is still to come).
    /// A gone task is released, or restored gone while it owes ends. Without
    /// `book` (an unreadable task log) no task entry is judged; an open one
    /// is restored only for the ends it owes. Kept undated ends are dated.
    pub(crate) fn restore(
        &mut self,
        entries: Vec<RunLedgerEntry>,
        book: Option<&TasksBook>,
    ) -> Vec<Restored> {
        let now = now_ms();
        let mut restored = Vec::new();
        for entry in entries {
            let key = key_of(&entry.key);
            let owed: Vec<_> = entry
                .owed()
                .iter()
                .map(|owed| OwedEnd {
                    episode: owed.episode,
                    snapshot: snapshot_of(key.clone(), &owed.end),
                })
                .collect();
            let end = match &entry.state {
                EntryState::Unreported { end, .. } => RestoredEnd::Unreported {
                    episode: end.episode,
                    snapshot: snapshot_of(key.clone(), &end.end),
                },
                EntryState::Reported { end, .. } => {
                    let snapshot = snapshot_of(key.clone(), &end.end);
                    match (&key, book) {
                        _ if !owed.is_empty() => RestoredEnd::Reported(snapshot),
                        (RunKey::Task(_), None) => RestoredEnd::Reported(snapshot),
                        (RunKey::Task(exec), Some(book))
                            if is_interruption(&snapshot) && holds(book, exec) =>
                        {
                            RestoredEnd::Reported(snapshot)
                        }
                        _ => {
                            self.release(&entry.key, ReleaseReason::Reported);
                            continue;
                        }
                    }
                }
                EntryState::Open { since_ms, .. } => match (&key, book) {
                    (RunKey::Task(_), None) => match owed.last() {
                        Some(owed) => RestoredEnd::Unjudged(owed.stand_in(RunState::Unknown)),
                        None => continue,
                    },
                    (RunKey::Task(exec), Some(book)) if !holds(book, exec) => match owed.last() {
                        Some(owed) => RestoredEnd::Gone(owed.stand_in(RunState::Gone)),
                        None => {
                            self.release(&entry.key, ReleaseReason::Expired);
                            continue;
                        }
                    },
                    (RunKey::Flow(_), _) if owed.is_empty() && lapsed(*since_ms, now) => {
                        self.lapse(&entry.key);
                        RestoredEnd::Open
                    }
                    _ => RestoredEnd::Open,
                },
            };
            self.date_undated(&entry, now);
            restored.push(Restored {
                key,
                solicited: entry.solicited,
                location: entry.location.as_ref().map(location_of),
                owed,
                end,
            });
        }
        restored
    }

    /// Date each end a log from before ends were dated left undated, so its
    /// directory hold runs from this restore. A re-record dates only an
    /// undated end, so this date is the one the end keeps.
    fn date_undated(&mut self, entry: &RunLedgerEntry, now: u64) {
        for recorded in entry.recorded().filter(|end| end.ended_at_ms.is_none()) {
            self.record(RunLedgerEvent::Ended {
                key: entry.key.clone(),
                solicited: entry.solicited,
                episode: recorded.episode,
                end: recorded.end.clone(),
                evicts: Vec::new(),
                ended_at_ms: Some(now),
            });
        }
    }

    pub(super) fn lapse(&mut self, key: &LedgerKey) {
        if let Self::Writing { lapsed, .. } = self {
            lapsed.insert(key.clone());
        }
    }
}

/// Whether a run open since `since_ms` has outlived the directory hold.
fn lapsed(since_ms: Option<u64>, now_ms: u64) -> bool {
    let hold_ms = u64::try_from(DIRECTORY_HOLD.as_millis()).unwrap_or(u64::MAX);
    since_ms.is_some_and(|since| now_ms.saturating_sub(since) >= hold_ms)
}

fn holds(book: &TasksBook, exec: &ExecutionRef) -> bool {
    book.get(&exec.task_id).is_some_and(|record| {
        record.current_exec() == Some(exec)
            || record.ended_executions.iter().any(|end| &end.exec == exec)
    })
}

#[cfg(test)]
mod tests;
