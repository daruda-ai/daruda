//! The root updates owed: what each pane thread's root should show next, what
//! it shows, and which edits are under way. GPUI-free; `roots` drives it.

use super::status::ShownStatus;
use super::{RootName, Scope};
use crate::remote_channel::transport::slack::threads::RootFailure;
use std::collections::{HashMap, HashSet};

/// A root on one connection.
type RootKey = (String, String);

/// What a root shows: the pane's name and its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RootFace {
    pub name: RootName,
    pub shown: ShownStatus,
}

#[derive(Clone, Debug)]
pub(super) struct RootUpdate {
    pub scope: Scope,
    /// The thread's id, which its stop button carries.
    pub id: String,
    pub root: String,
    pub face: RootFace,
}

impl RootUpdate {
    fn key(&self) -> RootKey {
        (self.scope.connection.clone(), self.root.clone())
    }
}

/// What a finished root edit leaves to do.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Finished {
    /// Shown; send whatever changed meanwhile.
    Shown,
    /// The root message is gone: forget its thread.
    Missing,
    /// Slack refused the edit itself. The thread is kept, so replies in it
    /// still route; the root keeps what it showed.
    Refused,
    /// Try again on the retry tick.
    Retry,
}

/// Where a root's next edit stands, when one is owed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    InFlight,
    /// Failed for a passing reason, or its connection could not send; sent
    /// again only on the retry tick, so a rate-limited channel is not pressed
    /// by every other root's change.
    Waiting,
}

#[derive(Default)]
pub(crate) struct Roots {
    /// The face each root should show next.
    desired: HashMap<RootKey, RootUpdate>,
    phase: HashMap<RootKey, Phase>,
    /// What each root shows now, as far as this run knows.
    shown: HashMap<RootKey, RootFace>,
    /// Roots Slack refused to edit: not asked again this run, since the same
    /// edit would be refused again.
    refused: HashSet<RootKey>,
}

impl Roots {
    /// Ask `root` to show `face`.
    pub(super) fn want(&mut self, scope: &Scope, id: &str, root: String, face: RootFace) {
        let key = (scope.connection.clone(), root.clone());
        if self.refused.contains(&key) {
            return;
        }
        self.desired.insert(
            key,
            RootUpdate {
                scope: scope.clone(),
                id: id.to_owned(),
                root,
                face,
            },
        );
    }

    /// Record that `root` was posted already showing `face`.
    pub(super) fn shown(&mut self, scope: &Scope, root: &str, face: RootFace) {
        self.shown
            .insert((scope.connection.clone(), root.to_owned()), face);
    }

    /// The updates to send now: wanted, not already shown, not in flight, and
    /// — unless this is the retry tick — not waiting.
    pub(super) fn ready(&mut self, retrying: bool) -> Vec<RootUpdate> {
        if retrying {
            self.phase.retain(|_, phase| *phase == Phase::InFlight);
        }
        // A root with an edit in flight is compared once that edit settles:
        // what it will show then is not known yet.
        let (shown, phase) = (&self.shown, &self.phase);
        self.desired.retain(|key, update| {
            phase.get(key) == Some(&Phase::InFlight) || shown.get(key) != Some(&update.face)
        });
        let ready: Vec<_> = self
            .desired
            .iter()
            .filter(|(key, _)| !self.phase.contains_key(*key))
            .map(|(_, update)| update.clone())
            .collect();
        for update in &ready {
            self.phase.insert(update.key(), Phase::InFlight);
        }
        ready
    }

    /// Record how `update`'s edit went.
    pub(super) fn finish(
        &mut self,
        update: &RootUpdate,
        result: &Result<(), RootFailure>,
    ) -> Finished {
        let key = update.key();
        self.phase.remove(&key);
        match result {
            Ok(()) => {
                self.shown.insert(key, update.face.clone());
                Finished::Shown
            }
            Err(RootFailure::Missing(_)) => {
                self.desired.remove(&key);
                self.shown.remove(&key);
                Finished::Missing
            }
            Err(RootFailure::Refused(_)) => {
                self.desired.remove(&key);
                self.refused.insert(key);
                Finished::Refused
            }
            Err(RootFailure::Transient(_)) => {
                self.phase.insert(key, Phase::Waiting);
                Finished::Retry
            }
        }
    }

    /// Put `update` back to wait for the retry tick: its connection cannot
    /// send right now. Expected while it reconnects, so not a failure.
    pub(super) fn hold(&mut self, update: &RootUpdate) {
        self.phase.insert(update.key(), Phase::Waiting);
    }

    /// Drop updates for scopes no connection serves any more: a removed
    /// channel, or a bot or chat that replaced the one the root belongs to.
    pub(super) fn keep_scopes(&mut self, alive: impl Fn(&Scope) -> bool) {
        self.desired.retain(|_, update| alive(&update.scope));
        let desired = &self.desired;
        self.phase
            .retain(|key, phase| *phase == Phase::InFlight || desired.contains_key(key));
    }

    /// The face `root` on `connection` is waiting to show, sent or not.
    #[cfg(test)]
    pub(super) fn wanted(&self, connection: &str, root: &str) -> Option<RootFace> {
        self.desired
            .get(&(connection.to_owned(), root.to_owned()))
            .map(|update| update.face.clone())
    }
}
