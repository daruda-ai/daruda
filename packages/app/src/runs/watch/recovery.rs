//! A new process takes back the scope and unreported ends the ledger kept.
use super::delivery::Delivery;
use super::state::{EndState, Membership, Origin, RunEvent, RunWatch};
use crate::runs::ledger::{Ledger, Restored, RestoredEnd};
use crate::runs::result::RunKey;

impl RunWatch {
    /// Write to `ledger` from now on and rejoin what it restored, before task
    /// history is compared: a restored end is then already known, so history
    /// retains it again without a second episode.
    pub(crate) fn attach_ledger(&mut self, ledger: Ledger, restored: Vec<Restored>) {
        self.ledger = ledger;
        for run in restored {
            self.rejoin(run);
        }
    }

    fn rejoin(&mut self, run: Restored) {
        if let (RunKey::Flow(id), Some(location)) = (&run.key, run.location) {
            self.locations.insert(id.clone(), location);
        }
        let entry = self.entry(&run.key);
        entry.membership = Membership::Scoped;
        entry.solicited = run.solicited;
        match run.end {
            RestoredEnd::Open => {}
            RestoredEnd::Reported(snapshot) => {
                entry.end = EndState::Reported;
                entry.snapshot = Some(snapshot);
            }
            RestoredEnd::Unreported { episode, snapshot } => {
                entry.end = EndState::Ended;
                entry.snapshot = Some(snapshot.clone());
                entry.last_sequence += 1;
                let event = RunEvent {
                    sequence: entry.last_sequence,
                    solicited: run.solicited,
                    origin: Origin::Recovered,
                    episode: entry.episode,
                    recorded: Some(episode),
                    snapshot,
                    delivery: Delivery::Unreported,
                };
                self.events.push(event.clone());
                self.announce(event);
            }
        }
    }
}

#[cfg(test)]
mod tests;
