//! Keeps each pane thread's root showing the pane's status. A root is edited
//! only when what it should show differs from what it shows, one edit at a
//! time per root; a failed edit is retried with the latest status.

use super::Scope;
use super::driver::{Threads, now, scope};
use super::status::{PaneActivity, ShownStatus};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::cards::EditFailure;
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::{self, slack};
use daruda_config::remote::RemoteRecipient;
use gpui::App;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A root on one connection.
type RootKey = (String, String);

#[derive(Clone, Debug)]
pub(super) struct RootUpdate {
    pub scope: Scope,
    /// The thread's id, which its stop button carries.
    pub id: String,
    pub root: String,
    pub title: String,
    pub shown: ShownStatus,
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
    /// The root is gone: forget its thread.
    Gone,
    /// Try again on the retry tick.
    Retry,
}

#[derive(Default)]
pub(crate) struct Roots {
    /// The status each root should show next.
    desired: HashMap<RootKey, RootUpdate>,
    in_flight: HashSet<RootKey>,
    /// What each root shows now, as far as this run knows.
    shown: HashMap<RootKey, ShownStatus>,
}

impl Roots {
    /// Ask `root` to show `shown` under `title`.
    pub(super) fn want(
        &mut self,
        scope: &Scope,
        id: &str,
        root: String,
        title: String,
        shown: ShownStatus,
    ) {
        self.desired.insert(
            (scope.connection.clone(), root.clone()),
            RootUpdate {
                scope: scope.clone(),
                id: id.to_owned(),
                root,
                title,
                shown,
            },
        );
    }

    /// Record that `root` was posted already showing `shown`.
    pub(super) fn shown(&mut self, scope: &Scope, root: &str, shown: ShownStatus) {
        self.shown
            .insert((scope.connection.clone(), root.to_owned()), shown);
    }

    /// The updates to send now: wanted, not already shown, not in flight.
    pub(super) fn ready(&mut self) -> Vec<RootUpdate> {
        let ready: Vec<_> = self
            .desired
            .iter()
            .filter(|(key, update)| {
                !self.in_flight.contains(*key) && self.shown.get(*key) != Some(&update.shown)
            })
            .map(|(_, update)| update.clone())
            .collect();
        self.desired
            .retain(|key, update| self.shown.get(key) != Some(&update.shown));
        for update in &ready {
            self.in_flight.insert(update.key());
        }
        ready
    }

    /// Record how `update`'s edit went.
    pub(super) fn finish(
        &mut self,
        update: &RootUpdate,
        result: &Result<(), EditFailure>,
    ) -> Finished {
        let key = update.key();
        self.in_flight.remove(&key);
        match result {
            Ok(()) => {
                self.shown.insert(key, update.shown);
                Finished::Shown
            }
            Err(EditFailure::Gone(_)) => {
                self.desired.remove(&key);
                self.shown.remove(&key);
                Finished::Gone
            }
            Err(EditFailure::Transient(_)) => Finished::Retry,
        }
    }

    /// The status `root` on `connection` is waiting to show, sent or not.
    #[cfg(test)]
    pub(super) fn wanted(&self, connection: &str, root: &str) -> Option<ShownStatus> {
        self.desired
            .get(&(connection.to_owned(), root.to_owned()))
            .map(|update| update.shown)
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

    /// Ask every root `pane` has in `scope` to show `shown`.
    pub(super) fn want_for_pane(&mut self, scope: &Scope, pane: PaneRef, shown: ShownStatus) {
        let Some(key) = self.last.get(&(scope.connection.clone(), pane)) else {
            return;
        };
        let (Some(record), Some(title)) = (
            self.book.for_conversation(scope, key),
            self.titles.get(&pane),
        ) else {
            return;
        };
        let (id, root, title) = (record.id.clone(), record.root.clone(), title.clone());
        self.roots.want(scope, &id, root, title, shown);
    }

    /// Forget a closed pane once its roots have been told.
    pub(super) fn forget_pane(&mut self, pane: PaneRef) {
        self.activity.remove(&pane);
        self.titles.remove(&pane);
        self.last.retain(|(_, held), _| *held != pane);
    }
}

/// Report `pane`'s activity. Roots of panes with no thread yet are untouched;
/// a later root starts out showing the latest status.
pub(crate) fn pane_activity(pane: PaneRef, activity: PaneActivity, cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let channels = cx.global_mut::<RemoteChannels>();
    // Reported on every pulse tick of a busy pane: an unchanged status costs
    // one lookup.
    let Some(shown) = channels.threads.note_activity(pane, activity, now()) else {
        return;
    };
    let scopes: Vec<Scope> = channels
        .connections
        .iter()
        .filter(|(_, connection)| {
            connection.can_send() && transport::pane_threads(connection.config.kind)
        })
        .filter_map(|(id, connection)| {
            Some(scope(
                id,
                connection.bot.as_ref()?,
                connection.config.recipient.as_ref()?,
            ))
        })
        .collect();
    let threads = &mut channels.threads;
    for scope in &scopes {
        threads.want_for_pane(scope, pane, shown);
    }
    if activity == PaneActivity::Closed {
        threads.forget_pane(pane);
    }
    flush(cx);
}

/// Send every root update that is due. Also the retry: called on the remote
/// channel's retry tick.
pub(crate) fn flush(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let ready = cx.global_mut::<RemoteChannels>().threads.roots.ready();
    for update in ready {
        cx.spawn(async move |cx| {
            let serving = cx.update(|cx| serving(&update.scope, cx));
            let result = match serving {
                Some((bot, recipient)) => {
                    let update = update.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let status = update.shown.view();
                            let stop = update.shown.is_running().then(|| {
                                (
                                    crate::surface::strings::remote_channel::thread_stop_button(),
                                    super::stop::token(&update.id),
                                )
                            });
                            slack::threads::replace_root(
                                &bot.credentials,
                                &recipient,
                                &update.root,
                                &slack::threads::RootView {
                                    title: &update.title,
                                    status: Some((status.0, &status.1)),
                                    stop: stop
                                        .as_ref()
                                        .map(|(label, value)| (label.as_str(), value.as_str())),
                                },
                            )
                        })
                        .await
                }
                None => Err(EditFailure::Transient(
                    "Remote channel is not serving".into(),
                )),
            };
            cx.update(|cx| settle(update, result, cx));
        })
        .detach();
    }
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

fn settle(update: RootUpdate, result: Result<(), EditFailure>, cx: &mut App) {
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    match threads.roots.finish(&update, &result) {
        Finished::Shown => flush(cx),
        // The root is gone; the pane's next message starts a new thread.
        Finished::Gone => {
            if let Err(EditFailure::Gone(reason)) = &result {
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
        // Not retried here: a channel that keeps failing would loop. The
        // retry tick sends the latest status again.
        Finished::Retry => {}
    }
}
