//! Sends the boards the pages are due off the main thread, one at a time
//! per page, and settles what each publish came to.

use super::super::board::Board;
use super::super::look::HomeLook;
use super::super::publish_queue::{Publication, Settled};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{AppHome, PageVerdict};
use gpui::App;
use std::sync::Arc;

/// Ask `scope`'s page to show `board`, and publish what is due.
pub(super) fn want(scope: &Scope, board: Board, cx: &mut App) {
    cx.global_mut::<RemoteChannels>()
        .home
        .queue
        .want(scope, board);
    flush(cx);
}

/// Publish every page that is due now.
fn flush(cx: &mut App) {
    let ready = cx.global_mut::<RemoteChannels>().home.queue.ready();
    for publication in ready {
        cx.spawn(async move |cx| {
            let serving = cx.update(|cx| serving(&publication.scope, cx));
            // Expected while the connection reconnects, so not a failure:
            // the first retry tick it can send on builds the page anew.
            let Some((bot, home, user_id)) = serving else {
                cx.update(|cx| {
                    cx.global_mut::<RemoteChannels>()
                        .home
                        .queue
                        .hold(&publication)
                });
                return;
            };
            let board = publication.board.clone();
            let verdict = cx
                .background_executor()
                .spawn(async move {
                    let look = HomeLook::new(&board);
                    (home.publish)(&bot.credentials, &user_id, &look.view())
                })
                .await;
            cx.update(|cx| settle(&publication, home, &verdict, cx));
        })
        .detach();
    }
}

/// The session that may publish the page made under `scope` now, the
/// channel's app home it writes to, and the person whose page it is: the
/// connection's live bot, if it can send and is still that identity.
pub(super) fn serving(scope: &Scope, cx: &App) -> Option<(Arc<BotSession>, AppHome, String)> {
    let connection = cx.global::<RemoteChannels>().sender(scope, cx)?;
    let home = super::app_home(connection.config.kind)?;
    let bot = connection.bot.clone()?;
    let user_id = connection.config.recipient.as_ref()?.user_id.clone();
    Some((bot, home, user_id))
}

fn settle(publication: &Publication, home: AppHome, verdict: &PageVerdict, cx: &mut App) {
    let queue = &mut cx.global_mut::<RemoteChannels>().home.queue;
    match (queue.finish(publication, verdict), verdict) {
        (Settled::Published, _) => flush(cx),
        (Settled::Unavailable { first: true }, _) => crate::remote_channel::log_error(
            "Remote home page is off",
            &home.off_hint,
            "remote.home.disabled",
        ),
        // Not retried here: a channel that keeps failing would loop. The
        // retry tick or the person's next opening builds the page anew.
        (Settled::Retry, PageVerdict::Transient(reason)) => crate::remote_channel::log_error(
            "Remote home page update will be retried",
            reason,
            "remote.home.retry",
        ),
        // A board wanted while the refused one was on its way differs from
        // it, so it goes now rather than on the next change; the refused
        // board itself is not sent again, so this cannot loop.
        (Settled::Dropped, PageVerdict::Refused(reason)) => {
            crate::remote_channel::log_error(
                "Remote home page refused an update",
                reason,
                "remote.home.refused",
            );
            flush(cx);
        }
        _ => {}
    }
}
