//! How each record moves the ledger's state.

use daruda_runlog::Machine;

use super::UNREPORTED_MAX;
use super::types::{
    EntryState, LedgerKey, LedgerRejected, RecordedEnd, RunLedgerEntry, RunLedgerEvent,
    RunLedgerState,
};

pub struct RunLedgerMachine;

impl Machine for RunLedgerMachine {
    type State = RunLedgerState;
    type Event = RunLedgerEvent;
    type Rejected = LedgerRejected;
    // Bump with any change to how `RunLedgerEvent` or `RunLedgerState`
    // serializes, so an older build refuses the log as too new.
    const SCHEMA: u32 = 5;

    fn check(state: &RunLedgerState, event: &RunLedgerEvent) -> Result<(), LedgerRejected> {
        match event {
            RunLedgerEvent::Owned { .. }
            | RunLedgerEvent::Reopened { .. }
            | RunLedgerEvent::Released { .. } => Ok(()),
            RunLedgerEvent::Ended { evicts, .. } => {
                if evicts.iter().any(|evicted| state.get(evicted).is_none()) {
                    return Err(LedgerRejected::InvalidEviction);
                }
                // A stale end is accepted and changes nothing: an evicted or
                // older episode can still gain text in memory after it lands.
                Ok(())
            }
            // Only a number no end has used is a proposer's bug. A report of
            // an older generation (released, evicted, resumed past) is stale
            // and, like a stale end, changes nothing.
            RunLedgerEvent::Reported { key, episode } => {
                if *episode >= state.next_episode {
                    Err(LedgerRejected::Unassigned {
                        key: key.clone(),
                        episode: *episode,
                    })
                } else {
                    Ok(())
                }
            }
        }
    }

    fn assign(state: &RunLedgerState, event: &mut RunLedgerEvent) {
        if let RunLedgerEvent::Ended {
            key,
            episode,
            evicts,
            ..
        } = event
            && matches!(state.landing(key, *episode), Landing::New)
        {
            *evicts = state.evictions();
        }
    }

    fn apply(state: &mut RunLedgerState, event: &RunLedgerEvent) {
        match event {
            RunLedgerEvent::Owned {
                key,
                solicited,
                location,
                at_ms,
            } => {
                let entry = state.entry_or(key, *solicited, || EntryState::Open {
                    owed: Vec::new(),
                    since_ms: *at_ms,
                });
                entry.solicited = *solicited;
                if location.is_some() {
                    entry.location.clone_from(location);
                }
            }
            RunLedgerEvent::Reopened {
                key,
                solicited,
                location,
                at_ms,
            } => {
                let entry = state.entry_or(key, *solicited, || EntryState::Open {
                    owed: Vec::new(),
                    since_ms: *at_ms,
                });
                entry.solicited = *solicited;
                if location.is_some() {
                    entry.location.clone_from(location);
                }
                entry.reopen(*at_ms);
            }
            RunLedgerEvent::Ended {
                key,
                solicited,
                episode,
                end,
                evicts,
                ended_at_ms,
            } => {
                let landing = state.landing(key, *episode);
                let recorded = RecordedEnd {
                    episode: *episode,
                    end: end.clone(),
                    ended_at_ms: *ended_at_ms,
                };
                match landing {
                    Landing::New => {
                        state.next_episode = episode + 1;
                        match state.get_mut(key) {
                            Some(entry) => entry.end(recorded),
                            None => state.entries.push(RunLedgerEntry::new(
                                key,
                                *solicited,
                                EntryState::Unreported {
                                    end: Box::new(recorded),
                                    owed: Vec::new(),
                                },
                            )),
                        }
                    }
                    Landing::Rerecord => {
                        if let Some(ended) = state
                            .get_mut(key)
                            .and_then(|entry| entry.recorded_mut(*episode))
                        {
                            ended.end = end.clone();
                            ended.ended_at_ms = ended.ended_at_ms.or(*ended_at_ms);
                        }
                    }
                    Landing::Stale => {}
                }
                // After the new end lands, so an earlier end of the same key
                // it displaced is already owed beside it.
                for evicted in evicts {
                    state.evict_oldest(evicted);
                }
            }
            // Episodes are ledger-wide, so a number the key no longer holds
            // (released, evicted, resumed past) matches nothing.
            RunLedgerEvent::Reported { key, episode } => {
                if let Some(entry) = state.get_mut(key) {
                    entry.report(*episode);
                }
            }
            RunLedgerEvent::Released { key, .. } => {
                state.entries.retain(|entry| &entry.key != key);
            }
        }
    }
}

enum Landing {
    New,
    Rerecord,
    Stale,
}

impl RunLedgerState {
    fn get_mut(&mut self, key: &LedgerKey) -> Option<&mut RunLedgerEntry> {
        self.entries.iter_mut().find(|entry| &entry.key == key)
    }

    /// How a proposed end lands: as a new episode, a re-record of one of the
    /// key's ends, or a stale end of an episode the ledger already let go.
    fn landing(&self, key: &LedgerKey, episode: u64) -> Landing {
        if episode >= self.next_episode {
            return Landing::New;
        }
        if self.get(key).is_some_and(|entry| entry.records(episode)) {
            Landing::Rerecord
        } else {
            Landing::Stale
        }
    }

    /// The entries whose oldest unreported ends to drop so a new end fits
    /// the cap, one mention per end. The ending key's own ends count too:
    /// they stay owed beside its new one.
    pub(super) fn evictions(&self) -> Vec<LedgerKey> {
        let mut unreported: Vec<_> = self
            .entries
            .iter()
            .flat_map(|entry| {
                entry
                    .unreported_ends()
                    .map(move |end| (end.episode, &entry.key))
            })
            .collect();
        let excess = (unreported.len() + 1).saturating_sub(UNREPORTED_MAX);
        unreported.sort_unstable_by_key(|(episode, _)| *episode);
        unreported
            .into_iter()
            .take(excess)
            .map(|(_, key)| key.clone())
            .collect()
    }

    fn evict_oldest(&mut self, key: &LedgerKey) {
        if let Some(index) = self.entries.iter().position(|entry| &entry.key == key)
            && !self.entries[index].evict_oldest()
        {
            self.entries.remove(index);
        }
    }

    /// The key's entry; a new one takes `solicited` and `state()`.
    fn entry_or(
        &mut self,
        key: &LedgerKey,
        solicited: bool,
        state: impl FnOnce() -> EntryState,
    ) -> &mut RunLedgerEntry {
        let index = match self.entries.iter().position(|entry| &entry.key == key) {
            Some(index) => index,
            None => {
                self.entries
                    .push(RunLedgerEntry::new(key, solicited, state()));
                self.entries.len() - 1
            }
        };
        &mut self.entries[index]
    }
}

impl RunLedgerEntry {
    fn new(key: &LedgerKey, solicited: bool, state: EntryState) -> Self {
        Self {
            key: key.clone(),
            solicited,
            location: None,
            state,
        }
    }

    /// The state, leaving an open placeholder for the caller to replace.
    fn take_state(&mut self) -> EntryState {
        std::mem::replace(
            &mut self.state,
            EntryState::Open {
                owed: Vec::new(),
                since_ms: None,
            },
        )
    }

    /// What the run still owes once its current state gives way: an
    /// unreported current end joins the owed ends as the newest.
    fn take_owed(&mut self) -> Vec<RecordedEnd> {
        match self.take_state() {
            EntryState::Open { owed, .. } | EntryState::Reported { owed, .. } => owed,
            EntryState::Unreported { end, mut owed } => {
                owed.push(*end);
                owed
            }
        }
    }

    /// The run ended again.
    fn end(&mut self, end: RecordedEnd) {
        let owed = self.take_owed();
        self.state = EntryState::Unreported {
            end: Box::new(end),
            owed,
        };
    }

    /// The run started again; every unreported end stays owed.
    fn reopen(&mut self, since_ms: Option<u64>) {
        let owed = self.take_owed();
        self.state = EntryState::Open { owed, since_ms };
    }

    /// Mark the recorded end numbered `episode`, and only that one, handed
    /// to an agent; any other number changes nothing.
    fn report(&mut self, episode: u64) {
        let state = match self.take_state() {
            EntryState::Unreported { end, owed } if end.episode == episode => {
                EntryState::Reported { end, owed }
            }
            mut state => {
                let (EntryState::Open { owed, .. }
                | EntryState::Unreported { owed, .. }
                | EntryState::Reported { owed, .. }) = &mut state;
                owed.retain(|owed| owed.episode != episode);
                state
            }
        };
        self.state = state;
    }

    /// The cap dropped this entry's oldest unreported end. An open run stays
    /// owned, and its directory held; a run whose only unreported end was
    /// its current one is let go. Whether the entry is kept.
    fn evict_oldest(&mut self) -> bool {
        match &mut self.state {
            EntryState::Unreported { owed, .. } if owed.is_empty() => false,
            EntryState::Open { owed, .. }
            | EntryState::Unreported { owed, .. }
            | EntryState::Reported { owed, .. } => {
                if !owed.is_empty() {
                    owed.remove(0);
                }
                true
            }
        }
    }

    /// The recorded end numbered `episode`, current or owed.
    fn recorded_mut(&mut self, episode: u64) -> Option<&mut RecordedEnd> {
        let (current, owed) = match &mut self.state {
            EntryState::Open { owed, .. } => (None, owed),
            EntryState::Unreported { end, owed } | EntryState::Reported { end, owed } => {
                (Some(&mut **end), owed)
            }
        };
        current
            .into_iter()
            .chain(owed.iter_mut())
            .find(|recorded| recorded.episode == episode)
    }

    fn records(&self, episode: u64) -> bool {
        self.recorded().any(|end| end.episode == episode)
    }
}

#[cfg(test)]
mod tests;
