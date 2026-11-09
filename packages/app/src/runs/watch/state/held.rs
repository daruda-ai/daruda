//! The flow pump holds a live end while it reads the run's report off-thread.

use super::{EndState, Origin, RunWatch};
use crate::runs::result::{RunKey, RunSnapshot, RunState};

/// The token a held end is released with; a later episode makes it stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HeldEnd {
    key: RunKey,
    episode: u64,
}

impl RunWatch {
    /// Retain an end whose announcement waits for [`Self::release_end`], so no
    /// waiter or notice receives it before its owner finishes enriching it.
    /// A later [`Self::retain`] of an end also releases it.
    pub(crate) fn hold_end(&mut self, snapshot: RunSnapshot) -> HeldEnd {
        let key = snapshot.key.clone();
        let entry = self.changed(&key);
        entry.disk_episode = None;
        if entry.end == EndState::Open {
            entry.end = EndState::Held;
        }
        let episode = entry.episode;
        entry.snapshot = Some(snapshot);
        self.clear_waiting(&key);
        self.wake();
        HeldEnd { key, episode }
    }
    /// The held end and its release token, while it waits for enrichment.
    pub(crate) fn held_end(&self, key: &RunKey) -> Option<(RunSnapshot, HeldEnd)> {
        let entry = self.entries.get(key)?;
        if entry.end != EndState::Held {
            return None;
        }
        let held = HeldEnd {
            key: key.clone(),
            episode: entry.episode,
        };
        Some((entry.snapshot.clone()?, held))
    }
    /// Announce a held end with its enriched content. A token whose episode
    /// [`Self::begin`] already closed only adds the text to that episode's
    /// unreported end; it never announces anything.
    pub(crate) fn release_end(&mut self, held: HeldEnd, enriched: RunSnapshot) {
        if enriched.key != held.key {
            return;
        }
        let Some(entry) = self.entries.get(&held.key) else {
            return;
        };
        if entry.episode != held.episode {
            self.enrich_closed(held, enriched);
            return;
        }
        if entry.end != EndState::Held {
            return;
        }
        let entry = self.changed(&held.key);
        entry.end = EndState::Ended;
        entry.snapshot = Some(enriched.clone());
        self.push_event(enriched, Origin::Live);
        self.wake();
    }

    /// Give an earlier episode's still-unreported end the text read for it,
    /// unless that end already has text.
    fn enrich_closed(&mut self, held: HeldEnd, enriched: RunSnapshot) {
        let RunState::Ended {
            text: Some(text), ..
        } = enriched.state
        else {
            return;
        };
        self.write_unreported_text(&held.key, &text, |event| {
            event.episode == held.episode
                && matches!(event.snapshot.state, RunState::Ended { text: None, .. })
        });
    }
}

#[cfg(test)]
mod tests;
