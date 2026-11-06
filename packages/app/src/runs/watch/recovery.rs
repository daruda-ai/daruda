//! A new process takes back the scope and unreported ends the ledger kept,
//! and judges the owned runs that ended, or stopped, while it was away.
use super::delivery::Delivery;
use super::state::{EndState, Judgement, Membership, Origin, RunEvent, RunWatch};
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

    /// Owned runs restored with no end that no read has judged yet: what a
    /// new process reads from disk once, before anyone asks.
    pub(crate) fn unjudged(&self) -> Vec<RunKey> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.judgement == Judgement::Pending)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Take the startup read of an owned run this process has not seen,
    /// unless another read judged it first. Its end or disappearance settles
    /// as any first read's does; anything else is left to later reads.
    pub(crate) fn judge_restored(&mut self, snapshot: RunSnapshot, disk_episode: Option<u64>) {
        let key = snapshot.key.clone();
        if self.entries.get(&key).map(|entry| entry.judgement) != Some(Judgement::Pending) {
            return;
        }
        match snapshot.state {
            RunState::Ended { .. } | RunState::Gone => self.settle(snapshot, Origin::Recovered),
            _ => {
                self.take_judgement(&key);
                self.after_judgement(&key, &snapshot.state);
                return;
            }
        }
        if let Some(episode) = disk_episode {
            self.entry(&key).disk_episode = Some(episode);
        }
    }

    /// Clear `key`'s pending judgement; whether one was pending.
    pub(super) fn take_judgement(&mut self, key: &RunKey) -> bool {
        self.entries
            .get_mut(key)
            .is_some_and(|entry| std::mem::take(&mut entry.judgement) == Judgement::Pending)
    }

    /// What a restored run's first read decides beyond what it retains. An
    /// end becomes a recovered episode (the caller's). A gone run can never
    /// end again: it leaves the ledger now, or, while ends it reached are
    /// still owed, once they are reported. Anything else stays owned, unless
    /// the ledger has kept the flow open past the directory hold: then it is
    /// let go.
    pub(super) fn after_judgement(&mut self, key: &RunKey, state: &RunState) {
        match state {
            RunState::Ended { .. } => {}
            RunState::Gone => {
                if !self.owes(key) {
                    self.ledger.expired(key);
                }
            }
            _ => {
                if self.ledger.release_if_lapsed(key) {
                    // Let go here too, so no wait over the scope waits on it.
                    self.entry(key).membership = Membership::Outside;
                }
            }
        }
    }

    /// Whether `snapshot` is the end the key's latest owed episode already
    /// says: a resume the restart cut off before it wrote anything leaves
    /// disk as it was. Read text is not compared; it may be read again.
    pub(super) fn repeats_owed_end(&self, snapshot: &RunSnapshot) -> bool {
        self.events
            .iter()
            .rev()
            .find(|event| event.snapshot.key == snapshot.key)
            .is_some_and(|owed| {
                owed.recorded.is_some()
                    && owed.superseded
                    && owed.delivery != Delivery::Reported
                    && same_end(&owed.snapshot, snapshot)
            })
    }

    /// Make the key's latest owed end its current one again, as if the
    /// resume that superseded it never began: no second episode is opened,
    /// and the run reads as resumable once more.
    pub(super) fn restore_owed_end(&mut self, key: &RunKey) {
        let Some(owed) = self
            .events
            .iter_mut()
            .rev()
            .find(|event| &event.snapshot.key == key)
        else {
            return;
        };
        owed.superseded = false;
        let (sequence, episode, snapshot) = (owed.sequence, owed.episode, owed.snapshot.clone());
        let entry = self.changed(key);
        entry.floor = sequence;
        entry.episode = episode;
        entry.end = if is_interruption(&snapshot) {
            EndState::Interrupted { reported: false }
        } else {
            EndState::Ended
        };
        entry.snapshot = Some(snapshot);
        self.wake();
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
        // Ends from before the run resumed, each its own episode of history
        // beside the current run.
        for OwedEnd { episode, snapshot } in run.owed {
            self.push_recovered(episode, snapshot);
            self.close_episode(&run.key);
        }
        let entry = self.entry(&run.key);
        match run.end {
            RestoredEnd::Open => entry.judgement = Judgement::Pending,
            // Known, so no startup read judges it and no wait waits on it.
            RestoredEnd::Unjudged(snapshot) | RestoredEnd::Gone(snapshot) => {
                entry.snapshot = Some(snapshot)
            }
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
            superseded: false,
        };
        self.events.push(event.clone());
        self.announce(event);
    }
}

/// Whether two ends say the same about one run, whatever text was read.
fn same_end(owed: &RunSnapshot, read: &RunSnapshot) -> bool {
    match (&owed.state, &read.state) {
        (
            RunState::Ended {
                ended_as: owed_as,
                pointers: owed_pointers,
                ..
            },
            RunState::Ended {
                ended_as: read_as,
                pointers: read_pointers,
                ..
            },
        ) => {
            owed.key == read.key
                && owed.what == read.what
                && owed.why == read.why
                && owed_as == read_as
                && owed_pointers == read_pointers
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
