//! Durable delivery of orchestrator run results across app restarts.
//!
//! Records which runs the orchestrator took into its scope, the end each one
//! reached as it would be delivered, and which of those ends were handed to an
//! agent. A reservation awaiting its socket receipt is never recorded: after a
//! restart it is simply unreported again, so it is delivered once more.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use daruda_runlog::Machine;
use serde::{Deserialize, Serialize};

use crate::tasks::ExecutionRef;

mod store;

pub use store::{Answered, RunLedgerError, RunLedgerStore};

/// Unreported ends the ledger keeps. A newer end beyond it evicts the oldest.
pub const UNREPORTED_MAX: usize = 200;

/// How long after its end an unreported flow run keeps its directory from
/// retention. The summary stays in the ledger until it is reported.
pub const DIRECTORY_HOLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The run a ledger entry follows; store-level twin of the app's run key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LedgerKey {
    Flow { run_id: String },
    Task(ExecutionRef),
}

/// Where a flow run's directory was, so a later process can read it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLocation {
    pub run_dir: PathBuf,
    #[serde(default)]
    pub lock_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerEndedAs {
    Completed,
    Failed,
    Canceled,
    Stalled,
    Crashed,
    Interrupted,
    Undelivered,
    Error,
    BudgetExhausted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerPointers {
    #[serde(default)]
    pub run_dir: Option<PathBuf>,
    #[serde(default)]
    pub report: Option<PathBuf>,
    #[serde(default)]
    pub outputs: Vec<PathBuf>,
    #[serde(default)]
    pub chat: Option<String>,
    #[serde(default)]
    pub transcript: Option<PathBuf>,
}

/// An end exactly as it would be delivered. `text` arrives already bounded
/// by the app's agent-text policy; the ledger stores it verbatim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEnd {
    pub what: String,
    #[serde(default)]
    pub why: Option<String>,
    pub ended_as: LedgerEndedAs,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub pointers: LedgerPointers,
}

/// An end and the ledger-wide episode number that identifies it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedEnd {
    pub episode: u64,
    pub end: LedgerEnd,
    /// When the proposer recorded the end, in Unix milliseconds; absent in
    /// logs written before ends were dated.
    #[serde(default)]
    pub ended_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLedgerEntry {
    pub key: LedgerKey,
    pub solicited: bool,
    #[serde(default)]
    pub location: Option<RunLocation>,
    /// The latest recorded end.
    #[serde(default)]
    pub ended: Option<RecordedEnd>,
    /// The newest episode handed to an agent; never above `ended`'s.
    #[serde(default)]
    pub reported: Option<u64>,
}

impl RunLedgerEntry {
    /// The latest end, while no agent has been handed it.
    pub fn unreported(&self) -> Option<&RecordedEnd> {
        self.ended.as_ref().filter(|ended| {
            self.reported
                .is_none_or(|reported| reported < ended.episode)
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLedgerState {
    #[serde(default)]
    next_episode: u64,
    /// In the order keys were first recorded; at most a few hundred.
    #[serde(default)]
    entries: Vec<RunLedgerEntry>,
}

impl RunLedgerState {
    pub fn entries(&self) -> &[RunLedgerEntry] {
        &self.entries
    }

    pub fn get(&self, key: &LedgerKey) -> Option<&RunLedgerEntry> {
        self.entries.iter().find(|entry| &entry.key == key)
    }

    /// Flow runs, by id, whose directories retention must spare at `now_ms`.
    pub fn held_run_dirs(&self, now_ms: u64) -> HashSet<String> {
        self.directory_holds().at(now_ms)
    }

    /// What [`Self::held_run_dirs`] reads, without the clock. A Task keeps
    /// its end in the ledger alone, so only flows are held. An owned run with
    /// no end yet is held too: it may have ended on disk before it was judged.
    pub(crate) fn directory_holds(&self) -> DirectoryHolds {
        let hold_ms = u64::try_from(DIRECTORY_HOLD.as_millis()).unwrap_or(u64::MAX);
        let holds = self
            .entries
            .iter()
            .filter_map(|entry| {
                let LedgerKey::Flow { run_id } = &entry.key else {
                    return None;
                };
                let until = match &entry.ended {
                    None => None,
                    Some(_) => entry
                        .unreported()?
                        .ended_at_ms
                        .map(|at| at.saturating_add(hold_ms)),
                };
                Some((run_id.clone(), until))
            })
            .collect();
        DirectoryHolds { holds }
    }

    /// The first episode number no recorded end has used.
    pub fn next_episode(&self) -> u64 {
        self.next_episode
    }

    fn get_mut(&mut self, key: &LedgerKey) -> Option<&mut RunLedgerEntry> {
        self.entries.iter_mut().find(|entry| &entry.key == key)
    }

    /// The key's entry; a new one takes `solicited`, an existing one keeps its own.
    fn entry(&mut self, key: &LedgerKey, solicited: bool) -> &mut RunLedgerEntry {
        let index = match self.entries.iter().position(|entry| &entry.key == key) {
            Some(index) => index,
            None => {
                self.entries.push(RunLedgerEntry {
                    key: key.clone(),
                    solicited,
                    location: None,
                    ended: None,
                    reported: None,
                });
                self.entries.len() - 1
            }
        };
        &mut self.entries[index]
    }

    /// How a proposed end lands: as a new episode, a re-record of the key's
    /// current end, or a stale end of an episode the ledger already let go.
    fn landing(&self, key: &LedgerKey, episode: u64) -> Landing {
        let current = self
            .get(key)
            .and_then(|entry| entry.ended.as_ref())
            .map(|ended| ended.episode);
        if current == Some(episode) {
            Landing::Rerecord
        } else if episode >= self.next_episode {
            Landing::New
        } else {
            Landing::Stale
        }
    }

    /// The oldest unreported ends to drop so `key`'s new end fits the cap.
    fn evictions(&self, key: &LedgerKey) -> Vec<LedgerKey> {
        let mut unreported: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| &entry.key != key)
            .filter_map(|entry| Some((entry.unreported()?.episode, &entry.key)))
            .collect();
        let excess = (unreported.len() + 1).saturating_sub(UNREPORTED_MAX);
        unreported.sort_unstable_by_key(|(episode, _)| *episode);
        unreported
            .into_iter()
            .take(excess)
            .map(|(_, key)| key.clone())
            .collect()
    }
}

/// Flow run directories the ledger holds, each with when its hold ends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirectoryHolds {
    /// `None`: no dated end, so held until the ledger lets the run go.
    holds: Vec<(String, Option<u64>)>,
}

impl DirectoryHolds {
    /// The run ids still held at `now_ms`.
    pub(crate) fn at(&self, now_ms: u64) -> HashSet<String> {
        self.holds
            .iter()
            .filter(|(_, until)| until.is_none_or(|until| now_ms < until))
            .map(|(run_id, _)| run_id.clone())
            .collect()
    }
}

enum Landing {
    New,
    Rerecord,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    /// The key's current end was handed to an agent.
    Reported,
    /// The run no longer exists, so nothing can ever be delivered for it.
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunLedgerEvent {
    /// The orchestrator took this run into its scope.
    Owned {
        key: LedgerKey,
        solicited: bool,
        #[serde(default)]
        location: Option<RunLocation>,
    },
    /// The run reached `end`. The proposer numbers episodes, since it tags its
    /// in-memory result with the number before the commit lands.
    Ended {
        key: LedgerKey,
        /// Used only when no `Owned` entry is left, e.g. after an eviction.
        solicited: bool,
        episode: u64,
        end: LedgerEnd,
        /// Filled by `assign`: the oldest unreported ends this one displaced.
        #[serde(default)]
        evicts: Vec<LedgerKey>,
        /// The proposer's clock, in Unix milliseconds, so replay never reads one.
        /// A re-record keeps the time of the end it replaces.
        #[serde(default)]
        ended_at_ms: Option<u64>,
    },
    /// An agent was handed `episode` and every earlier end of this key.
    Reported { key: LedgerKey, episode: u64 },
    Released {
        key: LedgerKey,
        reason: ReleaseReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerRejected {
    #[error("run {key:?} was reported at episode {episode} before that end was recorded")]
    NotEnded { key: LedgerKey, episode: u64 },
    #[error("a run end may only evict other recorded runs")]
    InvalidEviction,
}

pub struct RunLedgerMachine;

impl Machine for RunLedgerMachine {
    type State = RunLedgerState;
    type Event = RunLedgerEvent;
    type Rejected = LedgerRejected;
    // Bump with any change to how `RunLedgerEvent` or `RunLedgerState`
    // serializes, so an older build refuses the log as too new.
    const SCHEMA: u32 = 2;

    fn check(state: &RunLedgerState, event: &RunLedgerEvent) -> Result<(), LedgerRejected> {
        match event {
            RunLedgerEvent::Owned { .. } | RunLedgerEvent::Released { .. } => Ok(()),
            RunLedgerEvent::Ended { key, evicts, .. } => {
                if evicts
                    .iter()
                    .any(|evicted| evicted == key || state.get(evicted).is_none())
                {
                    return Err(LedgerRejected::InvalidEviction);
                }
                // A stale end is accepted and changes nothing: an evicted or
                // older episode can still gain text in memory after it lands.
                Ok(())
            }
            RunLedgerEvent::Reported { key, episode } => match state.get(key) {
                // Released or evicted: nothing is left to mark.
                None => Ok(()),
                Some(entry) if entry.ended.as_ref().is_some_and(|e| e.episode >= *episode) => {
                    Ok(())
                }
                Some(_) => Err(LedgerRejected::NotEnded {
                    key: key.clone(),
                    episode: *episode,
                }),
            },
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
            *evicts = state.evictions(key);
        }
    }

    fn apply(state: &mut RunLedgerState, event: &RunLedgerEvent) {
        match event {
            RunLedgerEvent::Owned {
                key,
                solicited,
                location,
            } => {
                let entry = state.entry(key, *solicited);
                entry.solicited = *solicited;
                if location.is_some() {
                    entry.location.clone_from(location);
                }
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
                state.entries.retain(|entry| !evicts.contains(&entry.key));
                match landing {
                    Landing::New => {
                        state.next_episode = episode + 1;
                        state.entry(key, *solicited).ended = Some(RecordedEnd {
                            episode: *episode,
                            end: end.clone(),
                            ended_at_ms: *ended_at_ms,
                        });
                    }
                    Landing::Rerecord => {
                        if let Some(ended) =
                            state.get_mut(key).and_then(|entry| entry.ended.as_mut())
                        {
                            ended.end = end.clone();
                            ended.ended_at_ms = ended.ended_at_ms.or(*ended_at_ms);
                        }
                    }
                    Landing::Stale => {}
                }
            }
            RunLedgerEvent::Reported { key, episode } => {
                if let Some(entry) = state.get_mut(key) {
                    entry.reported = Some(entry.reported.map_or(*episode, |r| r.max(*episode)));
                }
            }
            RunLedgerEvent::Released { key, .. } => {
                state.entries.retain(|entry| &entry.key != key);
            }
        }
    }
}

#[cfg(test)]
mod tests;
