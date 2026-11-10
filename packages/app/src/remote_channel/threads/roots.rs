//! Keeps each pane thread's root showing the pane's name and status. A root
//! is edited only when what it should show differs from what it shows, one
//! edit at a time per root; an edit that failed for a passing reason is
//! retried on the remote channel's retry tick with the latest face.

use super::status::{PaneActivity, ShownStatus};
use super::{RootName, Scope, now};
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
    pub name: RootName,
    pub shown: ShownStatus,
}

/// A root's face as Slack shows it: the status emoji and words over the
/// pane's name, and a stop button while there is work to stop. One builder
/// for the root's first post and every later edit, so the two cannot drift.
pub(super) struct RootLook {
    title: String,
    subject: Option<String>,
    separator: String,
    status: Option<(&'static str, String)>,
    stop: Option<(String, String)>,
}

impl RootLook {
    pub(super) fn new(name: &RootName, shown: Option<ShownStatus>, id: &str) -> Self {
        Self {
            title: name.title.clone(),
            subject: name.subject.clone(),
            separator: crate::surface::strings::remote_channel::thread_detail_separator(),
            status: shown.map(ShownStatus::view),
            stop: shown.filter(|shown| shown.is_running()).map(|_| {
                (
                    crate::surface::strings::remote_channel::thread_stop_button(),
                    super::stop::token(id),
                )
            }),
        }
    }

    pub(super) fn view(&self) -> slack::threads::RootView<'_> {
        slack::threads::RootView {
            title: &self.title,
            subject: self.subject.as_deref(),
            separator: &self.separator,
            status: self
                .status
                .as_ref()
                .map(|(emoji, label)| (*emoji, label.as_str())),
            stop: self
                .stop
                .as_ref()
                .map(|(label, value)| (label.as_str(), value.as_str())),
        }
    }
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
    /// Roots Slack refused to edit: not asked again this run, since the same
    /// edit would be refused again.
    refused: HashSet<RootKey>,
}

impl Roots {
    /// Ask `root` to show `face`.
    pub(super) fn want(&mut self, scope: &Scope, id: &str, root: String, face: RootFace) {
        if self
            .refused
            .contains(&(scope.connection.clone(), root.clone()))
        {
            return;
        }
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
                self.refused.insert(key);
                Finished::Refused
            }
            Err(RootFailure::Transient(_)) => {
                self.waiting.insert(key);
                Finished::Retry
            }
        }
    }

    /// Put `update` back to wait for the retry tick: its connection cannot
    /// send right now. Expected while it reconnects, so not a failure.
    pub(super) fn hold(&mut self, update: &RootUpdate) {
        let key = update.key();
        self.in_flight.remove(&key);
        self.waiting.insert(key);
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

/// Give `pane`'s roots its latest name; a root whose name changed is updated.
pub(crate) fn pane_named(pane: PaneRef, name: &RootName, cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    cx.global_mut::<RemoteChannels>()
        .threads
        .rename_pane(pane, name);
    flush(cx);
}

/// Send every root update that is due now.
pub(crate) fn flush(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let ready = cx
        .global_mut::<RemoteChannels>()
        .threads
        .roots()
        .ready(false);
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
            (Some(bot), Some(recipient)) => scopes.push(Scope::serving(id, bot, recipient)),
            // Reconnecting: its identity is not known yet, so keep its roots.
            _ => unsettled.push(id.clone()),
        }
    }
    channels.threads.forget_closed(now());
    let roots = channels.threads.roots();
    roots.keep_scopes(|scope| scopes.contains(scope) || unsettled.contains(&scope.connection));
    let ready = roots.ready(true);
    send(ready, cx);
}

fn send(ready: Vec<RootUpdate>, cx: &mut App) {
    for update in ready {
        cx.spawn(async move |cx| {
            let serving = cx.update(|cx| serving(&update.scope, cx));
            let Some((bot, recipient)) = serving else {
                cx.update(|cx| {
                    cx.global_mut::<RemoteChannels>()
                        .threads
                        .roots()
                        .hold(&update)
                });
                return;
            };
            let sent = update.clone();
            let result = cx
                .background_executor()
                .spawn(async move { replace(&bot, &recipient, &sent) })
                .await;
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
    let look = RootLook::new(&update.face.name, Some(update.face.shown), &update.id);
    slack::threads::replace_root(&bot.credentials, recipient, &update.root, &look.view())
}

/// The session that may edit roots made under `scope`: the connection's live
/// bot, if it is still the identity that made them.
fn serving(scope: &Scope, cx: &App) -> Option<(Arc<BotSession>, RemoteRecipient)> {
    let connection = cx.global::<RemoteChannels>().live(&scope.connection, cx)?;
    let bot = connection.bot.clone()?;
    let recipient = connection.config.recipient.clone()?;
    (connection.can_send() && Scope::serving(&scope.connection, &bot, &recipient) == *scope)
        .then_some((bot, recipient))
}

fn settle(update: RootUpdate, result: Result<(), RootFailure>, cx: &mut App) {
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    match threads.roots().finish(&update, &result) {
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
            threads.forget_root(&update.scope, &update.root);
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
