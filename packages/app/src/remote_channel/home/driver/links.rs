//! Asks for the links that open the threads a page shows. The board goes out
//! without the links not known yet; each one that lands builds the viewed
//! pages again, so the first rows link without waiting for the last.

use super::super::board::Board;
use super::super::publish_queue::{Learned, LinkAsk, PageEpoch};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{AppHome, LinkFailure};
use gpui::App;
use std::sync::Arc;

/// Ask `bot` for the links to the threads `board` shows without one that
/// `scope`'s page has not asked for yet. One at a time, off the main thread.
pub(super) fn ask(scope: &Scope, home: AppHome, bot: Arc<BotSession>, board: &Board, cx: &mut App) {
    let Some(LinkAsk { epoch, roots }) = cx
        .global_mut::<RemoteChannels>()
        .home
        .queue
        .ask_links(scope, board.unlinked())
    else {
        return;
    };
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
            // The page was replaced or forgotten: the rest of these asks
            // were its own, and a new page asks for itself.
            if !cx.update(|cx| learn(&scope, epoch, &root, link, cx)) {
                return;
            }
        }
    })
    .detach();
}

/// Keep what the link `scope`'s page of `epoch` asked for `root` came to.
/// Whether that page is still there.
fn learn(
    scope: &Scope,
    epoch: PageEpoch,
    root: &str,
    link: Result<String, LinkFailure>,
    cx: &mut App,
) -> bool {
    let queue = &mut cx.global_mut::<RemoteChannels>().home.queue;
    let Some(learned) = queue.learn_link(scope, epoch, root, &link) else {
        return false;
    };
    match (learned, link) {
        (Learned::Known, _) => super::changed(cx),
        // The row shows no button meanwhile; the retry tick or the person's
        // next opening asks again, and a repeat failure is not logged again.
        (Learned::Failed { first: true }, Err(error)) => crate::remote_channel::log_error(
            "Remote home thread link unavailable",
            &error,
            "remote.home.link",
        ),
        // Never asked again for this page, so told once.
        (Learned::Refused, Err(error)) => crate::remote_channel::log_error(
            "Remote home thread link refused",
            &error,
            "remote.home.link.refused",
        ),
        (Learned::Failed { .. } | Learned::Refused, _) => {}
    }
    true
}
