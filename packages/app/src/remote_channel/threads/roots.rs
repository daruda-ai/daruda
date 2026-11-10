//! Keeps each pane thread's root showing the pane's name and status. A root
//! is edited only when what it should show differs from what it shows, one
//! edit at a time per root; an edit that failed for a passing reason is
//! retried on the remote channel's retry tick with the latest face.

use super::Scope;
use super::driver::{Threads, now, scope};
use super::status::{PaneActivity, ShownStatus};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::slack::{self, threads::RootFailure};
use daruda_config::remote::RemoteRecipient;
use gpui::App;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A root on one connection.
type RootKey = (String, String);

/// What a root shows: the pane's name and its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RootFace {
    pub title: String,
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

#[derive(Default)]
pub(crate) struct Roots {
    /// The face each root should show next.
    desired: HashMap<RootKey, RootUpdate>,
    in_flight: HashSet<RootKey>,
    /// Failed for a passing reason; sent again only on the retry tick, so a
    /// rate-limited channel is not pressed by every other root's change.
    waiting: HashSet<RootKey>,
    /// What each root shows now, as far as this run knows.
    shown: HashMap<RootKey, RootFace>,
}

impl Roots {
    /// Ask `root` to show `face`.
    pub(super) fn want(&mut self, scope: &Scope, id: &str, root: String, face: RootFace) {
        self.desired.insert(
            (scope.connection.clone(), root.clone()),
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
    /// — unless this is the retry tick — not waiting out a failure.
    pub(super) fn ready(&mut self, retrying: bool) -> Vec<RootUpdate> {
        if retrying {
            self.waiting.clear();
        }
        let shown = &self.shown;
        self.desired
            .retain(|key, update| shown.get(key) != Some(&update.face));
        let ready: Vec<_> = self
            .desired
            .iter()
            .filter(|(key, _)| !self.in_flight.contains(*key) && !self.waiting.contains(*key))
            .map(|(_, update)| update.clone())
            .collect();
        for update in &ready {
            self.in_flight.insert(update.key());
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
        self.in_flight.remove(&key);
        self.waiting.remove(&key);
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
                Finished::Refused
            }
            Err(RootFailure::Transient(_)) => {
                self.waiting.insert(key);
                Finished::Retry
            }
        }
    }

    /// Drop updates for scopes no connection serves any more: a removed
    /// channel, or a bot or chat that replaced the one the root belongs to.
    fn keep_scopes(&mut self, alive: impl Fn(&Scope) -> bool) {
        self.desired.retain(|_, update| alive(&update.scope));
        self.waiting.retain(|key| self.desired.contains_key(key));
    }

    /// The face `root` on `connection` is waiting to show, sent or not.
    #[cfg(test)]
    pub(super) fn wanted(&self, connection: &str, root: &str) -> Option<RootFace> {
        self.desired
            .get(&(connection.to_owned(), root.to_owned()))
            .map(|update| update.face.clone())
    }
}

impl Threads {
    /// Note `pane`'s activity. The status its roots should now show, or
    /// `None` when nothing changed.
    pub(super) fn note_activity(
        &mut self,
        pane: PaneRef,
        activity: PaneActivity,
        now: i64,
    ) -> Option<ShownStatus> {
        let previous = self.activity.get(&pane).copied();
        let shown = ShownStatus::next(previous, activity, now);
        if previous == Some(shown) {
            return None;
        }
        self.activity.insert(pane, shown);
        Some(shown)
    }

    /// Ask every root `pane` has to show `shown`. Found through the threads
    /// the pane posted in this run, whatever state their connections are in:
    /// a connection that cannot send now is caught up on its retry tick.
    pub(super) fn want_for_pane(&mut self, pane: PaneRef, shown: ShownStatus) {
        let wanted: Vec<_> = self
            .last
            .iter()
            .filter(|((_, held), _)| *held == pane)
            .filter_map(|((connection, _), key)| {
                let record = self.book.for_connection(connection, key)?;
                let title = self.titles.get(&(connection.clone(), pane))?;
                Some((
                    Scope::of(record),
                    record.id.clone(),
                    record.root.clone(),
                    title.clone(),
                ))
            })
            .collect();
        for (scope, id, root, title) in wanted {
            self.roots
                .want(&scope, &id, root, RootFace { title, shown });
        }
    }

    /// Forget where a closed pane posted. Its `Closed` status is kept, so a
    /// root still being posted for it is closed as soon as it lands.
    pub(super) fn forget_pane(&mut self, pane: PaneRef) {
        self.last.retain(|(_, held), _| *held != pane);
        self.titles.retain(|(_, held), _| *held != pane);
    }
}

/// Report `pane`'s activity. Panes with no thread yet only remember it, for
/// the root a later message starts.
pub(crate) fn pane_activity(pane: PaneRef, activity: PaneActivity, cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    // Reported on every pulse tick of a busy pane: an unchanged status costs
    // one lookup.
    let Some(shown) = threads.note_activity(pane, activity, now()) else {
        return;
    };
    threads.want_for_pane(pane, shown);
    if activity == PaneActivity::Closed {
        threads.forget_pane(pane);
    }
    flush(cx);
}

/// Send every root update that is due now.
pub(crate) fn flush(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let ready = cx.global_mut::<RemoteChannels>().threads.roots.ready(false);
    send(ready, cx);
}

/// The retry tick: drop updates no connection can make any more, then send
/// the rest, including those that failed for a passing reason.
pub(crate) fn retry(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let channels = cx.global_mut::<RemoteChannels>();
    let mut scopes = Vec::new();
    let mut unsettled = Vec::new();
    for (id, connection) in &channels.connections {
        match (&connection.bot, &connection.config.recipient) {
            (Some(bot), Some(recipient)) => scopes.push(scope(id, bot, recipient)),
            // Reconnecting: its identity is not known yet, so keep its roots.
            _ => unsettled.push(id.clone()),
        }
    }
    let roots = &mut channels.threads.roots;
    roots.keep_scopes(|scope| scopes.contains(scope) || unsettled.contains(&scope.connection));
    let ready = roots.ready(true);
    send(ready, cx);
}

fn send(ready: Vec<RootUpdate>, cx: &mut App) {
    for update in ready {
        cx.spawn(async move |cx| {
            let serving = cx.update(|cx| serving(&update.scope, cx));
            let result = match serving {
                Some((bot, recipient)) => {
                    let update = update.clone();
                    cx.background_executor()
                        .spawn(async move { replace(&bot, &recipient, &update) })
                        .await
                }
                None => Err(RootFailure::Transient(
                    "Remote channel is not serving".into(),
                )),
            };
            cx.update(|cx| settle(update, result, cx));
        })
        .detach();
    }
}

/// Show `update` on its root. Blocking.
fn replace(
    bot: &BotSession,
    recipient: &RemoteRecipient,
    update: &RootUpdate,
) -> Result<(), RootFailure> {
    let (emoji, label) = update.face.shown.view();
    let stop = update.face.shown.is_running().then(|| {
        (
            crate::surface::strings::remote_channel::thread_stop_button(),
            super::stop::token(&update.id),
        )
    });
    slack::threads::replace_root(
        &bot.credentials,
        recipient,
        &update.root,
        &slack::threads::RootView {
            title: &update.face.title,
            status: Some((emoji, &label)),
            stop: stop
                .as_ref()
                .map(|(label, value)| (label.as_str(), value.as_str())),
        },
    )
}

/// The session that may edit roots made under `scope`: the connection's live
/// bot, if it is still the identity that made them.
fn serving(scope: &Scope, cx: &App) -> Option<(Arc<BotSession>, RemoteRecipient)> {
    let connection = cx.global::<RemoteChannels>().live(&scope.connection, cx)?;
    let bot = connection.bot.clone()?;
    let recipient = connection.config.recipient.clone()?;
    (connection.can_send() && super::driver::scope(&scope.connection, &bot, &recipient) == *scope)
        .then_some((bot, recipient))
}

fn settle(update: RootUpdate, result: Result<(), RootFailure>, cx: &mut App) {
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    match threads.roots.finish(&update, &result) {
        Finished::Shown => flush(cx),
        // The root is gone; the pane's next message starts a new thread.
        Finished::Missing => {
            if let Err(RootFailure::Missing(reason)) = &result {
                crate::remote_channel::log_error(
                    "Remote thread root is gone",
                    reason,
                    "remote.thread.root_gone",
                );
            }
            if threads.book.forget_root(&update.scope, &update.root) {
                threads.save();
            }
        }
        Finished::Refused => {
            if let Err(RootFailure::Refused(reason)) = &result {
                crate::remote_channel::log_error(
                    "Remote thread root refused an update",
                    reason,
                    "remote.thread.root_refused",
                );
            }
        }
        // Not retried here: a channel that keeps failing would loop. The
        // retry tick sends the latest face again.
        Finished::Retry => {
            if let Err(RootFailure::Transient(reason)) = &result {
                crate::remote_channel::log_error(
                    "Remote thread root update will be retried",
                    reason,
                    "remote.thread.root_retry",
                );
            }
        }
    }
}
