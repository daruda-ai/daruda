//! Asks for the links that open the threads a page shows, so the board
//! carries their buttons.

use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::AppHome;
use gpui::App;
use std::sync::Arc;

/// Ask `scope`'s page to show the panes as they are. Thread links not known
/// yet are asked of `bot` first, so the board carries their buttons.
pub(super) fn publish_latest(
    scope: Scope,
    home: AppHome,
    bot: Option<Arc<BotSession>>,
    cx: &mut App,
) {
    let board = super::board(&scope, home.capacity, cx);
    let mut unlinked: Vec<String> = board.unlinked().map(str::to_owned).collect();
    unlinked.sort();
    unlinked.dedup();
    let Some(bot) = bot.filter(|_| !unlinked.is_empty()) else {
        super::publish::want(&scope, board, cx);
        return;
    };
    cx.spawn(async move |cx| {
        let credentials = bot.credentials.clone();
        let channel = scope.conversation.clone();
        let fetched = cx
            .background_executor()
            .spawn(async move {
                unlinked
                    .into_iter()
                    .map(|root| {
                        let link = (home.link)(&credentials, &channel, &root);
                        (root, link)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        cx.update(|cx| {
            let home_state = &mut cx.global_mut::<RemoteChannels>().home;
            for (root, link) in fetched {
                match link {
                    Ok(link) => home_state.learn_link(&scope, root, link),
                    // Not remembered: the next build asks again, and the row
                    // shows no button meanwhile.
                    Err(error) => crate::remote_channel::log_error(
                        "Remote home thread link unavailable",
                        &error,
                        "remote.home.permalink",
                    ),
                }
            }
            // Built again: the panes may have changed while the links were
            // asked for, and the latest board wins.
            let board = super::board(&scope, home.capacity, cx);
            super::publish::want(&scope, board, cx);
        });
    })
    .detach();
}
