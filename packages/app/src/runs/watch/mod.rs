//! Process-lifetime query scope, delivery ledger and retained execution results.
mod delivery;
mod history;
mod observation;
mod recovery;
mod retention;
mod state;
pub(crate) use delivery::{WaitAnswer, WaitId, WaitOutcome, WaiterBusy};
pub(crate) use observation::LocalEnd;
pub(super) use state::RunWatch;
pub(crate) use state::{HeldEnd, Origin, RunEvent};
#[cfg(test)]
mod tests;
