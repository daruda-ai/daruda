//! Drives the root updates `root_queue` owes: takes the host's reports of
//! pane activity and names, sends due edits off the main thread, and settles
//! what they came to. A root is edited only when what it should show differs
//! from what it shows, one edit at a time; an edit that failed for a passing
//! reason is retried on the remote channel's retry tick.

use super::look::RootLook;
use super::root_queue::{Finished, RootUpdate};
use super::status::PaneActivity;
use super::{RootName, Scope, now};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::transport::slack::{self, threads::RootFailure};
use daruda_config::remote::RemoteRecipient;
use gpui::App;
use std::sync::Arc;

/// Report `pane`'s activity, and whether the status it shows changed. Panes
/// with no thread yet only remember it, for the root a later message starts.
pub(in crate::remote_channel) fn pane_activity(
    pane: PaneRef,
    activity: PaneActivity,
    cx: &mut App,
) -> bool {
    if !cx.has_global::<RemoteChannels>() {
        return false;
    }
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    // Reported on every pulse tick of a busy pane: an unchanged status costs
    // one lookup.
    let Some(shown) = threads.note_activity(pane, activity, now()) else {
        return false;
    };
    threads.want_for_pane(pane, shown);
    if activity == PaneActivity::Closed {
        threads.forget_pane(pane);
    }
    flush(cx);
    true
}

/// Give `pane`'s roots its latest name; a root whose name changed is updated.
pub(in crate::remote_channel) fn pane_named(pane: PaneRef, name: &RootName, cx: &mut App) {
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
    let ready = cx.global_mut::<RemoteChannels>().threads.due_updates(false);
    send(ready, cx);
}

/// The retry tick: drop updates no connection can make any more, then send
/// the rest, including those that failed for a passing reason.
pub(crate) fn retry(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let alive = cx.global::<RemoteChannels>().scope_alive(cx);
    let channels = cx.global_mut::<RemoteChannels>();
    channels.threads.forget_closed(now());
    channels.threads.keep_live_scopes(alive);
    let ready = channels.threads.due_updates(true);
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
                        .hold_update(&update)
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
    (connection.can_send() && connection.thread_scope(&bot).as_ref() == Some(scope))
        .then_some((bot, recipient))
}

fn settle(update: RootUpdate, result: Result<(), RootFailure>, cx: &mut App) {
    let threads = &mut cx.global_mut::<RemoteChannels>().threads;
    match threads.settle_update(&update, &result) {
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
