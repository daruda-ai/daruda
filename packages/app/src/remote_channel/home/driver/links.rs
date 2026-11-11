//! Asks for the links that open the threads a page shows. The board goes out
//! without the links not known yet; each one that lands builds the viewed
//! pages again, so the first rows link without waiting for the last.

use super::super::board::Board;
use super::super::publish_queue::Learned;
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{AppHome, TransportError};
use gpui::App;
use std::sync::Arc;

/// Ask `bot` for the links to the threads `board` shows without one that
/// `scope`'s page has not asked for yet. One at a time, off the main thread.
pub(super) fn ask(scope: &Scope, home: AppHome, bot: Arc<BotSession>, board: &Board, cx: &mut App) {
    let roots = cx
        .global_mut::<RemoteChannels>()
        .home
        .queue
        .ask_links(scope, board.unlinked());
    if roots.is_empty() {
        return;
    }
    let scope = scope.clone();
    cx.spawn(async move |cx| {
        for root in roots {
            let credentials = bot.credentials.clone();
            let channel = scope.conversation.clone();
            let asked = root.clone();
            let link = cx
                .background_executor()
                .spawn(async move { (home.link)(&credentials, &channel, &asked) })
                .await;
            // Another identity's page has replaced this one: its own asks
            // are its own.
            if !cx.update(|cx| learn(&scope, &root, link, cx)) {
                return;
            }
        }
    })
    .detach();
}

/// Keep what the link asked for `root` came to. Whether `scope`'s page
/// still waited for it.
fn learn(scope: &Scope, root: &str, link: Result<String, TransportError>, cx: &mut App) -> bool {
    let queue = &mut cx.global_mut::<RemoteChannels>().home.queue;
    match (
        queue.learn_link(scope, root, link.as_ref().ok().cloned()),
        link,
    ) {
        (Learned::Stale, _) => return false,
        (Learned::Known, _) => super::changed(cx),
        // The row shows no button meanwhile; the retry tick or the person's
        // next opening asks again, and a repeat failure is not logged again.
        (Learned::Failed { first: true }, Err(error)) => crate::remote_channel::log_error(
            "Remote home thread link unavailable",
            &error,
            "remote.home.permalink",
        ),
        (Learned::Failed { .. }, _) => {}
    }
    true
}
