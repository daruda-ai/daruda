//! Durable delivery of orchestrator run results across app restarts.
//!
//! Records which runs the orchestrator took into its scope, the end each one
//! reached as it would be delivered, and which of those ends were handed to an
//! agent. A reservation awaiting its socket receipt is never recorded: after a
//! restart it is simply unreported again, so it is delivered once more.

use std::time::Duration;

mod machine;
mod store;
mod types;

pub use machine::RunLedgerMachine;
pub use store::{Answered, LedgerBarrier, MarkHold, RunLedgerError, RunLedgerStore};
pub(crate) use types::DirectoryHolds;
pub use types::{
    EntryState, LedgerEnd, LedgerEndedAs, LedgerKey, LedgerPointers, LedgerRejected, RecordedEnd,
    ReleaseReason, RunLedgerEntry, RunLedgerEvent, RunLedgerState, RunLocation,
};

/// Unreported ends the ledger keeps. A newer end beyond it evicts the oldest.
pub const UNREPORTED_MAX: usize = 200;

/// How long after its end an unreported flow run keeps its directory from
/// retention. The summary stays in the ledger until it is reported.
pub const DIRECTORY_HOLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[cfg(test)]
mod tests;
