//! Storage is independent of windows and orchestrator sessions.
use super::delivery::{Delivery, WaitId, Waiter};
use crate::runs::result::{
    FlowRunLocation, RunKey, RunSnapshot, RunState, WaitingOn, is_interruption,
};
use std::collections::HashMap;

/// Whether the delivery ledger records a key's episodes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Membership {
    /// Observed only: no waiter or notice can receive its episodes.
    #[default]
    Outside,
    /// Named by an explicit waiter without joining the omitted-handles scope.
    Enrolled,
    /// Started by the orchestrator in this process.
    Scoped,
}

/// How far the current episode's end has travelled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum EndState {
    #[default]
    Open,
    /// Ended, but announced only once its owner releases the enriched result.
    Held,
    Ended,
    /// The current episode's end was handed to an agent.
    Reported,
    /// A running Task's flow stopped where a resume picks it up. The Task's
    /// own end, whenever it comes, is the next episode, not a replacement.
    Interrupted {
        reported: bool,
    },
}

impl EndState {
    /// Whether the current episode has an end announced or reported.
    pub(super) fn has_end(self) -> bool {
        !matches!(self, Self::Open | Self::Held)
    }

    /// Whether the current episode's end was handed to an agent.
    pub(super) fn is_reported(self) -> bool {
        matches!(self, Self::Reported | Self::Interrupted { reported: true })
    }

    /// The state `snapshot`'s end leaves its episode in: a running Task's
    /// flow interruption keeps its own, so the Task's end can follow it.
    pub(super) fn of_end(snapshot: &RunSnapshot, reported: bool) -> Self {
        match (is_interruption(snapshot), reported) {
            (true, reported) => Self::Interrupted { reported },
            (false, false) => Self::Ended,
            (false, true) => Self::Reported,
        }
    }

    /// The state once the current episode's end is handed to an agent.
    pub(super) fn reported(self) -> Self {
        match self {
            Self::Interrupted { .. } => Self::Interrupted { reported: true },
            _ => Self::Reported,
        }
    }
}

/// Whether a run restored open from the ledger still awaits this process's
/// first read of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Judgement {
    /// Read by this process, started in it, or never restored open.
    #[default]
    Done,
    /// Restored open and not read yet: whatever its first read shows, from
    /// whichever path, was reached before the restart.
    Pending,
}

/// An open block and the ledger episode that announced it, if one was recorded.
#[derive(Clone, Debug)]
pub(super) struct Block {
    on: WaitingOn,
    pub(super) sequence: Option<u64>,
}

/// Everything the ledger knows about one run key.
#[derive(Default)]
pub(super) struct RunEntry {
    pub(super) membership: Membership,
    pub(super) solicited: bool,
    pub(super) snapshot: Option<RunSnapshot>,
    pub(super) end: EndState,
    pub(super) block: Option<Block>,
    /// Allocated from the watch's clock so forgotten keys cannot reuse revisions.
    pub(super) revision: u64,
    pub(super) episode: u64,
    /// The last issued event number while this key is retained.
    pub(super) last_sequence: u64,
    /// The first episode number of the current run; lower ones are history.
    pub(super) floor: u64,
    pub(super) disk_episode: Option<u64>,
    pub(super) local_sequence: Option<u64>,
    /// When the key was first kept only to remember its end was reported.
    pub(super) retired: Option<u64>,
    pub(super) judgement: Judgement,
}

impl RunEntry {
    fn new(revision: u64) -> Self {
        Self {
            revision,
            episode: revision,
            ..Self::default()
        }
    }

    pub(super) fn blocked_sequence(&self) -> Option<u64> {
        self.block.as_ref().and_then(|block| block.sequence)
    }

    /// Whether the key is kept only to remember that its end was reported. A
    /// scoped run that is not over (a Task's reported flow interruption) has
    /// its own end still to come, which only this entry can announce.
    pub(super) fn only_remembers_report(&self) -> bool {
        self.end.is_reported() && (self.membership != Membership::Scoped || self.is_over())
    }

    /// Whether nothing of the run is left to end: its end was reported, or
    /// the run no longer exists.
    pub(super) fn is_over(&self) -> bool {
        match self.end {
            EndState::Reported => true,
            EndState::Open => self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| matches!(snapshot.state, RunState::Gone)),
            EndState::Held | EndState::Ended | EndState::Interrupted { .. } => false,
        }
    }
}

/// The token a held end is released with; a later episode makes it stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HeldEnd {
    key: RunKey,
    episode: u64,
}

#[derive(Default)]
pub(crate) struct RunWatch {
    pub(super) entries: HashMap<RunKey, RunEntry>,
    pub(in crate::runs) event_sender: Option<smol::channel::Sender<RunEvent>>,
    pub(super) locations: HashMap<String, FlowRunLocation>,
    pub(super) events: Vec<RunEvent>,
    pub(super) waiters: HashMap<WaitId, Waiter>,
    pub(super) next_waiter: u64,
    /// Orders retirements so the oldest reported keys are forgotten first.
    pub(super) retirements: u64,
    pub(super) revisions: u64,
    pub(super) next_sweep: usize,
    /// What must outlive the process: scope, unreported ends and their reports.
    pub(in crate::runs) ledger: crate::runs::ledger::Ledger,
}
/// Whether an episode arose in this process or was restored from the ledger.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Origin {
    #[default]
    Live,
    /// Requested before the app restarted and not yet handed to an agent.
    Recovered,
}

/// A process-local episode with a stable per-key identity.
/// The channel may announce it again when waiter coverage is withdrawn.
#[derive(Clone, Debug)]
pub(crate) struct RunEvent {
    pub(crate) sequence: u64,
    pub(crate) solicited: bool,
    pub(crate) origin: Origin,
    pub(super) episode: u64,
    /// The ledger's episode for a recorded end, which its report names.
    pub(super) recorded: Option<u64>,
    pub(crate) snapshot: RunSnapshot,
    pub(super) delivery: Delivery,
    /// A later episode of the key began: a resume picked this end's run up,
    /// or the Task's own end followed its flow's interruption.
    pub(super) superseded: bool,
}
impl RunEvent {
    /// Whether the run this end names can be picked up now: a resumable end
    /// no later episode has superseded, so an owed crash is never re-offered.
    pub(crate) fn resumable(&self) -> bool {
        !self.superseded && self.snapshot.resumable()
    }
}
#[cfg(test)]
impl RunEvent {
    pub(crate) fn for_test(sequence: u64, snapshot: RunSnapshot) -> Self {
        Self {
            sequence,
            solicited: false,
            origin: Origin::Live,
            episode: 0,
            recorded: None,
            snapshot,
            delivery: Delivery::Unreported,
            superseded: false,
        }
    }
}
impl RunWatch {
    pub(super) fn entry(&mut self, key: &RunKey) -> &mut RunEntry {
        self.entries.entry(key.clone()).or_insert_with(|| {
            self.revisions += 1;
            RunEntry::new(self.revisions)
        })
    }
    pub(super) fn changed(&mut self, key: &RunKey) -> &mut RunEntry {
        self.revisions += 1;
        let revision = self.revisions;
        let entry = self
            .entries
            .entry(key.clone())
            .or_insert_with(|| RunEntry::new(revision));
        entry.revision = revision;
        entry
    }
    pub(super) fn membership(&self, key: &RunKey) -> Membership {
        self.entries
            .get(key)
            .map(|entry| entry.membership)
            .unwrap_or_default()
    }
    pub(super) fn floor(&self, key: &RunKey) -> u64 {
        self.entries.get(key).map_or(0, |entry| entry.floor)
    }
    /// A resumed flow can end again with the same external run handle.
    pub(crate) fn revision(&self, key: &RunKey) -> u64 {
        self.entries
            .get(key)
            .map_or(self.revisions, |entry| entry.revision)
    }
    /// Open a new episode, keeping the key's solicitation.
    pub(crate) fn begin(&mut self, key: &RunKey) {
        self.begin_with(key, None);
    }

    /// Open a new episode, taking `solicitation` when the start brings its
    /// own. An end still held for enrichment is announced first, as the
    /// closing episode's, so a resume during the report read cannot lose it.
    pub(crate) fn begin_with(&mut self, key: &RunKey, solicitation: Option<bool>) {
        let entry = self.entry(key);
        if entry.end == EndState::Held
            && let Some(held) = entry.snapshot.clone()
        {
            entry.end = EndState::Ended;
            self.push_event(held, Origin::Live);
        }
        let entry = self.changed(key);
        if let Some(solicited) = solicitation {
            entry.solicited = solicited;
        }
        entry.disk_episode = None;
        entry.local_sequence = None;
        entry.snapshot = None;
        entry.end = EndState::Open;
        // A start in this process is no end from before the restart.
        entry.judgement = Judgement::Done;
        self.close_episode(key);
        if self.membership(key) == Membership::Scoped {
            // A resumed run is owned again, even after its last end was released,
            // and an end it never delivered stays owed.
            self.record_reopened(key);
        }
        self.clear_waiting(key);
        self.wake();
    }

    /// Start the key's next episode: earlier events become its history.
    pub(super) fn close_episode(&mut self, key: &RunKey) {
        let entry = self.changed(key);
        entry.episode = entry.revision;
        entry.floor = entry.last_sequence + 1;
        for event in &mut self.events {
            if &event.snapshot.key == key {
                event.superseded = true;
            }
        }
    }
    pub(crate) fn waiting_on(&self, key: &RunKey) -> Option<&WaitingOn> {
        self.entries.get(key)?.block.as_ref().map(|block| &block.on)
    }
    #[cfg(test)]
    pub(crate) fn pending(&self, key: &RunKey) -> Option<&RunEvent> {
        self.events
            .iter()
            .rev()
            .find(|event| &event.snapshot.key == key)
    }
    fn clear_waiting(&mut self, key: &RunKey) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.block = None;
        }
        self.events.retain(|event| {
            &event.snapshot.key != key
                || !matches!(event.snapshot.state, RunState::Running { .. })
                || event.delivery != Delivery::Unreported
        });
    }
    /// Queue an end or block this process saw. An observed-only run queues
    /// nothing, and only a scoped one's episode is recorded in the ledger.
    fn push_event(&mut self, snapshot: RunSnapshot, origin: Origin) {
        let Some(entry) = self.entries.get(&snapshot.key) else {
            return;
        };
        let recorded = match entry.membership {
            Membership::Outside => return,
            Membership::Enrolled => None,
            Membership::Scoped => self.ledger.ended(&snapshot, entry.solicited),
        };
        let blocked_on = match &snapshot.state {
            RunState::Running {
                waiting_on: Some(on),
                ..
            } => Some(on.clone()),
            _ => None,
        };
        let key = snapshot.key.clone();
        let sequence = self.queue_event(snapshot, origin, recorded);
        if let Some(on) = blocked_on {
            let block = self
                .entry(&key)
                .block
                .get_or_insert(Block { on, sequence: None });
            block.sequence = Some(sequence);
        }
        self.wake();
    }

    /// Append `snapshot` as its key's next unreported event and announce it;
    /// the event's sequence. Whether the key may queue one is the caller's.
    pub(super) fn queue_event(
        &mut self,
        snapshot: RunSnapshot,
        origin: Origin,
        recorded: Option<u64>,
    ) -> u64 {
        let entry = self.entry(&snapshot.key);
        entry.last_sequence += 1;
        let event = RunEvent {
            sequence: entry.last_sequence,
            solicited: entry.solicited,
            origin,
            episode: entry.episode,
            recorded,
            snapshot,
            delivery: Delivery::Unreported,
            superseded: false,
        };
        let sequence = event.sequence;
        self.events.push(event.clone());
        self.announce(event);
        sequence
    }
    /// Only scoped episodes can become notices; enrolled ones wait for their waiter.
    pub(super) fn announce(&self, event: RunEvent) {
        if self.membership(&event.snapshot.key) != Membership::Scoped {
            return;
        }
        if let Some(sender) = &self.event_sender {
            // SILENT-OK: the receiver closes only when the app/effects are replaced.
            let _ = sender.try_send(event);
        }
    }
    pub(crate) fn observe_waiting(&mut self, snapshot: RunSnapshot) {
        let RunState::Running { waiting_on, .. } = &snapshot.state else {
            return;
        };
        let key = snapshot.key.clone();
        if self.get(&key) == Some(&snapshot) {
            return;
        }
        if self.take_judgement(&key) {
            self.after_judgement(&key, &snapshot.state);
        }
        let entry = self.changed(&key);
        match waiting_on {
            Some(waiting) => {
                if let Some(block) = &mut entry.block {
                    block.on = waiting.clone();
                    if let Some(event) = self.events.iter_mut().rev().find(|event| {
                        event.snapshot.key == key
                            && event.delivery == Delivery::Unreported
                            && matches!(event.snapshot.state, RunState::Running { .. })
                    }) {
                        event.snapshot = snapshot.clone();
                    }
                } else {
                    entry.block = Some(Block {
                        on: waiting.clone(),
                        sequence: None,
                    });
                    self.push_event(snapshot.clone(), Origin::Live);
                }
            }
            None => self.clear_waiting(&key),
        }
        self.entry(&key).snapshot = Some(snapshot);
        self.bound();
        self.wake();
    }
    pub(crate) fn locate(&mut self, location: FlowRunLocation) {
        if let RunKey::Flow(id) = RunKey::flow_of(&location.run_dir) {
            self.locations.insert(id, location);
        }
    }
    pub(crate) fn location(&self, id: &str) -> Option<&FlowRunLocation> {
        self.locations.get(id)
    }
    pub(crate) fn register(&mut self, key: RunKey) {
        self.register_solicited(key, false);
    }

    pub(crate) fn register_solicited(&mut self, key: RunKey, solicited: bool) {
        let was = self.membership(&key);
        let entry = self.entry(&key);
        let changed = was != Membership::Scoped || entry.solicited != solicited;
        entry.membership = Membership::Scoped;
        entry.solicited = solicited;
        if changed {
            self.record_owned(&key);
        }
        match was {
            Membership::Outside => self.announce_cached(&key),
            Membership::Enrolled => self.record_scoped_ends(&key),
            Membership::Scoped => {}
        }
    }
    pub(super) fn record_owned(&mut self, key: &RunKey) {
        let (solicited, location) = self.ownership(key);
        self.ledger.owned(key, solicited, location.as_ref());
    }

    fn record_reopened(&mut self, key: &RunKey) {
        let (solicited, location) = self.ownership(key);
        self.ledger.reopened(key, solicited, location.as_ref());
    }

    fn ownership(&self, key: &RunKey) -> (bool, Option<FlowRunLocation>) {
        let solicited = self.entries.get(key).is_some_and(|entry| entry.solicited);
        let location = match key {
            RunKey::Flow(id) => self.locations.get(id).cloned(),
            RunKey::Task(_) => None,
        };
        (solicited, location)
    }
    /// Ends an explicit waiter enrolled before the key joined the scope.
    fn record_scoped_ends(&mut self, key: &RunKey) {
        for event in &mut self.events {
            if &event.snapshot.key == key
                && event.recorded.is_none()
                && event.delivery != Delivery::Reported
            {
                event.recorded = self.ledger.ended(&event.snapshot, event.solicited);
            }
        }
    }
    pub(super) fn enroll(&mut self, key: RunKey) {
        if self.membership(&key) == Membership::Outside {
            self.entry(&key).membership = Membership::Enrolled;
            self.announce_cached(&key);
        }
    }
    /// A key joining the ledger turns its retained, unreported state into an episode.
    fn announce_cached(&mut self, key: &RunKey) {
        let Some(entry) = self.entries.get(key) else {
            return;
        };
        let Some(snapshot) = entry.snapshot.clone() else {
            return;
        };
        let announce = match snapshot.state {
            RunState::Ended { .. } => {
                matches!(
                    entry.end,
                    EndState::Ended | EndState::Interrupted { reported: false }
                )
            }
            RunState::Running {
                waiting_on: Some(_),
                ..
            } => true,
            _ => false,
        };
        if announce {
            self.push_event(snapshot, Origin::Live);
        }
    }
    /// Leave the ledger once no explicit waiter names the key. Unreported episodes
    /// are dropped: no one can receive them, and a later enrollment recreates them.
    pub(super) fn release_enrollment(&mut self, key: &RunKey) {
        if self.membership(key) != Membership::Enrolled
            || self.waiters.values().any(|waiter| waiter.names(key))
            || self.events.iter().any(|event| {
                &event.snapshot.key == key && matches!(event.delivery, Delivery::InFlight(_))
            })
        {
            return;
        }
        let entry = self.entry(key);
        entry.membership = Membership::Outside;
        if let Some(block) = &mut entry.block {
            block.sequence = None;
        }
        self.events
            .retain(|event| &event.snapshot.key != key || event.delivery != Delivery::Unreported);
    }
    pub(crate) fn in_scope(&self, key: &RunKey) -> bool {
        self.membership(key) == Membership::Scoped
    }
    pub(crate) fn scope(&self) -> Vec<RunKey> {
        let mut keys: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.membership == Membership::Scoped)
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys
    }
    pub(crate) fn get(&self, key: &RunKey) -> Option<&RunSnapshot> {
        self.entries.get(key)?.snapshot.as_ref()
    }
    #[cfg(test)]
    pub(super) fn retained_count(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| entry.snapshot.is_some())
            .count()
    }
    /// Text enrichment is not another lifecycle event or an outcome change.
    pub(crate) fn enrich(&mut self, snapshot: RunSnapshot) {
        let RunState::Ended {
            text: Some(text), ..
        } = snapshot.state
        else {
            return;
        };
        let entry = self.changed(&snapshot.key);
        let floor = entry.floor;
        if let Some(current) = &mut entry.snapshot
            && let RunState::Ended {
                text: current_text, ..
            } = &mut current.state
        {
            *current_text = Some(text.clone());
        }
        self.write_unreported_text(&snapshot.key, &text, |event| event.sequence >= floor);
    }

    /// Give `key`'s unreported ends that `takes` accepts `text`, and record
    /// each again so a restart delivers it with that text.
    fn write_unreported_text(
        &mut self,
        key: &RunKey,
        text: &str,
        takes: impl Fn(&RunEvent) -> bool,
    ) {
        for event in &mut self.events {
            if &event.snapshot.key == key
                && event.delivery == Delivery::Unreported
                && takes(event)
                && let RunState::Ended {
                    text: current_text, ..
                } = &mut event.snapshot.state
            {
                *current_text = Some(text.to_owned());
                if let Some(episode) = event.recorded {
                    self.ledger
                        .rerecord(episode, &event.snapshot, event.solicited);
                }
            }
        }
    }
    /// Retain an observation. An end already held with identical content is
    /// no change, so in-flight reads and waiters are left alone.
    pub(crate) fn retain(&mut self, snapshot: RunSnapshot) {
        self.settle(snapshot, Origin::Live);
    }

    /// [`Self::retain`], with the origin a new end's episode takes. The first
    /// read of a restored run is its judgement, whichever path made it.
    pub(super) fn settle(&mut self, snapshot: RunSnapshot, origin: Origin) {
        let judged = self.take_judgement(&snapshot.key);
        if judged && self.repeats_owed_end(&snapshot) {
            self.restore_owed_end(&snapshot.key);
            return;
        }
        let origin = if judged { Origin::Recovered } else { origin };
        let ended = matches!(snapshot.state, RunState::Ended { .. });
        let interruption = is_interruption(&snapshot);
        let entry = self.entry(&snapshot.key);
        if ended && entry.end.has_end() && entry.snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        if ended && !interruption && matches!(entry.end, EndState::Interrupted { .. }) {
            // The Task's own end after its flow's interruption, resumed or not.
            self.begin(&snapshot.key);
        }
        let entry = self.changed(&snapshot.key);
        entry.disk_episode = None;
        if ended {
            let first = matches!(entry.end, EndState::Open | EndState::Held);
            if first {
                entry.end = EndState::of_end(&snapshot, false);
            }
            self.clear_waiting(&snapshot.key);
            if first {
                self.push_event(snapshot.clone(), origin);
            }
        }
        let key = snapshot.key.clone();
        if judged {
            self.after_judgement(&key, &snapshot.state);
        }
        self.entry(&key).snapshot = Some(snapshot);
        self.bound();
        self.wake();
    }
    pub(super) fn wake(&self) {
        for waiter in self.waiters.values() {
            // SILENT-OK: a full channel already holds a wake-up; a closed one has no waiter.
            let _ = waiter.wake.try_send(());
        }
    }
}
// The flow pump holds a live end while it reads the run's report off-thread.
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
