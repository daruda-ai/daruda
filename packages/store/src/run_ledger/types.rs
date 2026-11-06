//! What the ledger records and replays to, and the queries its readers make.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

use super::DIRECTORY_HOLD;
use crate::tasks::ExecutionRef;

/// The run a ledger entry follows; store-level twin of the app's run key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// When the proposer first dated the end, in Unix milliseconds; absent
    /// in logs written before ends were dated, until a re-record dates it.
    #[serde(default)]
    pub ended_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLedgerEntry {
    pub key: LedgerKey,
    pub solicited: bool,
    #[serde(default)]
    pub location: Option<RunLocation>,
    pub state: EntryState,
}

/// Where a run stands, and what of it no agent has been handed yet. Each
/// `owed` list holds the run's earlier ends no agent has been handed, oldest
/// first and all older than its current end; the cap bounds them ledger-wide.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryState {
    /// Running with no end recorded since it was owned or reopened.
    Open {
        /// Ends of the run before it resumed, still owed to an agent.
        #[serde(default, deserialize_with = "owed_ends")]
        owed: Vec<RecordedEnd>,
        /// When it was owned or reopened, in Unix milliseconds; absent when
        /// the proposer gave no time.
        #[serde(default)]
        since_ms: Option<u64>,
    },
    /// Ended, and no agent has been handed this end yet.
    Unreported {
        end: Box<RecordedEnd>,
        #[serde(default, deserialize_with = "owed_ends")]
        owed: Vec<RecordedEnd>,
    },
    /// Its latest end was handed to an agent.
    Reported {
        end: Box<RecordedEnd>,
        #[serde(default, deserialize_with = "owed_ends")]
        owed: Vec<RecordedEnd>,
    },
}

/// Schema 4 snapshots kept at most one owed end, as `null` or an end; later
/// ones keep a list. A value of any other shape is named in the error.
fn owed_ends<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<RecordedEnd>, D::Error> {
    struct Owed;

    impl<'de> Visitor<'de> for Owed {
        type Value = Vec<RecordedEnd>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the owed ends: null, one recorded end, or a list of recorded ends")
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }

        fn visit_some<D: Deserializer<'de>>(self, inner: D) -> Result<Self::Value, D::Error> {
            inner.deserialize_any(self)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut ends = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(end) = seq.next_element()? {
                ends.push(end);
            }
            Ok(ends)
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            let end = RecordedEnd::deserialize(de::value::MapAccessDeserializer::new(map))?;
            Ok(vec![end])
        }
    }

    deserializer.deserialize_any(Owed)
}

impl RunLedgerEntry {
    /// The run's latest recorded end, reported or not.
    pub fn latest(&self) -> Option<&RecordedEnd> {
        match &self.state {
            EntryState::Open { owed, .. } => owed.last(),
            EntryState::Unreported { end, .. } | EntryState::Reported { end, .. } => Some(end),
        }
    }

    /// The newest end no agent has been handed.
    pub fn unreported(&self) -> Option<&RecordedEnd> {
        self.unreported_ends().last()
    }

    /// Every end no agent has been handed, oldest first.
    pub fn unreported_ends(&self) -> impl Iterator<Item = &RecordedEnd> {
        let current = match &self.state {
            EntryState::Unreported { end, .. } => Some(&**end),
            EntryState::Open { .. } | EntryState::Reported { .. } => None,
        };
        self.owed().iter().chain(current)
    }

    /// The run's earlier ends still owed beside its current state, oldest first.
    pub fn owed(&self) -> &[RecordedEnd] {
        match &self.state {
            EntryState::Open { owed, .. }
            | EntryState::Unreported { owed, .. }
            | EntryState::Reported { owed, .. } => owed,
        }
    }

    /// Every end the entry records, current and owed, in no particular order.
    pub fn recorded(&self) -> impl Iterator<Item = &RecordedEnd> {
        let current = match &self.state {
            EntryState::Open { .. } => None,
            EntryState::Unreported { end, .. } | EntryState::Reported { end, .. } => Some(&**end),
        };
        current.into_iter().chain(self.owed())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLedgerState {
    #[serde(default)]
    pub(super) next_episode: u64,
    /// In the order keys were first recorded; at most a few hundred.
    #[serde(default)]
    pub(super) entries: Vec<RunLedgerEntry>,
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
    /// its end in the ledger alone, so only flows are held. An open run is
    /// held too: it may have ended on disk before it was judged.
    pub(crate) fn directory_holds(&self) -> DirectoryHolds {
        let hold_ms = u64::try_from(DIRECTORY_HOLD.as_millis()).unwrap_or(u64::MAX);
        let holds = self
            .entries
            .iter()
            .filter_map(|entry| {
                let LedgerKey::Flow { run_id } = &entry.key else {
                    return None;
                };
                // The newest owed deadline; an undated end holds until release.
                let until = match &entry.state {
                    EntryState::Open { .. } => None,
                    EntryState::Reported { owed, .. } if owed.is_empty() => return None,
                    EntryState::Unreported { .. } | EntryState::Reported { .. } => {
                        entry.unreported_ends().try_fold(0, |until: u64, end| {
                            end.ended_at_ms
                                .map(|at| until.max(at.saturating_add(hold_ms)))
                        })
                    }
                };
                Some(Hold {
                    run_id: run_id.clone(),
                    run_dir: entry.location.as_ref().map(|at| at.run_dir.clone()),
                    until,
                })
            })
            .collect();
        DirectoryHolds { holds }
    }

    /// The first episode number no recorded end has used.
    pub fn next_episode(&self) -> u64 {
        self.next_episode
    }
}

/// Flow run directories the ledger holds, each with when its hold ends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DirectoryHolds {
    holds: Vec<Hold>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Hold {
    run_id: String,
    /// Where the run's directory is, when its owner recorded it.
    run_dir: Option<PathBuf>,
    /// `None`: no dated end, so held until the ledger lets the run go.
    until: Option<u64>,
}

impl Hold {
    /// The deadline a marker in the run's directory carries, if any.
    fn marked(&self) -> Option<(&Path, u64)> {
        Some((self.run_dir.as_deref()?, self.until?))
    }
}

impl DirectoryHolds {
    /// The run ids still held at `now_ms`.
    pub(crate) fn at(&self, now_ms: u64) -> HashSet<String> {
        self.holds
            .iter()
            .filter(|hold| hold.until.is_none_or(|until| now_ms < until))
            .map(|hold| hold.run_id.clone())
            .collect()
    }

    /// How the dated holds in `next` differ from these, per run directory:
    /// `Some(until)` for one newly held or held to a new deadline, `None` for
    /// one no longer held with a deadline.
    pub(crate) fn marker_changes(&self, next: &Self) -> Vec<(PathBuf, Option<u64>)> {
        let before: Vec<_> = self.holds.iter().filter_map(Hold::marked).collect();
        let after: Vec<_> = next.holds.iter().filter_map(Hold::marked).collect();
        let cleared = before
            .iter()
            .filter(|(dir, _)| !after.iter().any(|(next, _)| next == dir))
            .map(|(dir, _)| (dir.to_path_buf(), None));
        let marked = after
            .iter()
            .filter(|held| !before.contains(held))
            .map(|(dir, until)| (dir.to_path_buf(), Some(*until)));
        cleared.chain(marked).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
    /// Every recorded end of the key was handed to an agent.
    Reported,
    /// The run no longer exists, so nothing can ever be delivered for it.
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunLedgerEvent {
    /// The orchestrator took this run into its scope. An entry it already
    /// has only takes the new solicitation and location.
    Owned {
        key: LedgerKey,
        solicited: bool,
        #[serde(default)]
        location: Option<RunLocation>,
        /// The proposer's clock, in Unix milliseconds.
        #[serde(default)]
        at_ms: Option<u64>,
    },
    /// An owned run started again, as a resume does: the entry is open once
    /// more and an end still unreported stays owed. Owns a released run.
    Reopened {
        key: LedgerKey,
        solicited: bool,
        #[serde(default)]
        location: Option<RunLocation>,
        #[serde(default)]
        at_ms: Option<u64>,
    },
    /// The run reached `end`. The proposer numbers episodes, since it tags its
    /// in-memory result with the number before the commit lands.
    Ended {
        key: LedgerKey,
        /// Used only when no `Owned` entry is left, e.g. after an eviction.
        solicited: bool,
        episode: u64,
        end: LedgerEnd,
        /// Filled by `assign`: the entries whose oldest unreported end this
        /// one displaced, once per end, so a key can appear more than once.
        #[serde(default)]
        evicts: Vec<LedgerKey>,
        /// The proposer's clock, in Unix milliseconds, so replay never reads one.
        /// A re-record never moves a date: it only dates an undated end.
        #[serde(default)]
        ended_at_ms: Option<u64>,
    },
    /// An agent was handed the end numbered `episode`, and only that one.
    Reported { key: LedgerKey, episode: u64 },
    Released {
        key: LedgerKey,
        reason: ReleaseReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LedgerRejected {
    #[error("run {key:?} was reported at episode {episode}, which no end has used")]
    Unassigned { key: LedgerKey, episode: u64 },
    #[error("a run end may only evict the ends of recorded runs")]
    InvalidEviction,
}

#[cfg(test)]
mod tests;
