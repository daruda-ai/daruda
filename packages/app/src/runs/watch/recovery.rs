//! A new process takes back the scope and unreported ends the ledger kept,
//! and judges the owned runs that ended, or stopped, while it was away.
use super::delivery::Delivery;
use super::state::{EndState, Membership, Origin, RunEvent, RunWatch};
use crate::runs::ledger::{Ledger, OwedEnd, Restored, RestoredEnd};
use crate::runs::result::{RunKey, RunSnapshot, RunState, is_interruption};

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

    /// Owned runs with no end and nothing observed yet: what a new process
    /// reads from disk once, before anyone asks.
    pub(crate) fn unjudged(&self) -> Vec<RunKey> {
        self.entries
            .iter()
            .filter(|(_, entry)| {
                entry.membership == Membership::Scoped
                    && entry.end == EndState::Open
                    && entry.snapshot.is_none()
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Take what disk says about an owned run this process has not seen. An
    /// end becomes a recovered episode: it was asked for before the restart.
    /// A flow whose directory is gone can never deliver and leaves the
    /// ledger. Anything else is left to later reads, unless the ledger has
    /// kept the flow open past the directory hold: then it is let go.
    pub(crate) fn judge_restored(&mut self, snapshot: RunSnapshot, disk_episode: Option<u64>) {
        let key = snapshot.key.clone();
        match snapshot.state {
            RunState::Ended { .. } => self.settle(snapshot, Origin::Recovered),
            RunState::Gone if matches!(key, RunKey::Flow(_)) => {
                self.retain(snapshot);
                self.ledger.expired(&key);
            }
            _ => {
                if self.ledger.release_if_lapsed(&key) {
                    // Let go here too, so no wait over the scope waits on it.
                    self.entry(&key).membership = Membership::Outside;
                }
                return;
            }
        }
        if let Some(episode) = disk_episode {
            self.entry(&key).disk_episode = Some(episode);
        }
    }

    /// A run that ended resumable started again: its key takes a new episode,
    /// keeping its solicitation, so the run's real end is reported too.
    pub(crate) fn reopen(&mut self, key: &RunKey) {
        if self
            .entries
            .get(key)
            .is_some_and(|entry| entry.end != EndState::Open)
        {
            self.begin(key);
        }
    }

    fn rejoin(&mut self, run: Restored) {
        if let (RunKey::Flow(id), Some(location)) = (&run.key, run.location) {
            self.locations.insert(id.clone(), location);
        }
        let entry = self.entry(&run.key);
        entry.membership = Membership::Scoped;
        entry.solicited = run.solicited;
        if let Some(OwedEnd { episode, snapshot }) = run.owed {
            // An end from before the run resumed: history beside the current run.
            self.push_recovered(episode, snapshot);
            self.close_episode(&run.key);
        }
        let entry = self.entry(&run.key);
        match run.end {
            RestoredEnd::Open => {}
            RestoredEnd::Reported(snapshot) => {
                entry.end = if is_interruption(&snapshot) {
                    EndState::Interrupted { reported: true }
                } else {
                    EndState::Reported
                };
                entry.snapshot = Some(snapshot);
            }
            RestoredEnd::Unreported { episode, snapshot } => {
                entry.end = if is_interruption(&snapshot) {
                    EndState::Interrupted { reported: false }
                } else {
                    EndState::Ended
                };
                entry.snapshot = Some(snapshot.clone());
                self.push_recovered(episode, snapshot);
            }
        }
    }

    /// Queue a restored, unreported end as a recovered episode of its key.
    fn push_recovered(&mut self, recorded: u64, snapshot: RunSnapshot) {
        let entry = self.entry(&snapshot.key);
        entry.last_sequence += 1;
        let event = RunEvent {
            sequence: entry.last_sequence,
            solicited: entry.solicited,
            origin: Origin::Recovered,
            episode: entry.episode,
            recorded: Some(recorded),
            snapshot,
            delivery: Delivery::Unreported,
        };
        self.events.push(event.clone());
        self.announce(event);
    }
}

#[cfg(test)]
mod tests;
