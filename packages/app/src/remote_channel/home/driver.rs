//! Drives the pages `publish_queue` owes: takes the person opening a page
//! and the panes' changes, lists the panes with their status and thread
//! links, publishes off the main thread, and settles what that came to.

use super::board::{Board, PaneRow};
use super::look::HomeLook;
use super::publish_queue::{Publication, Settled};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{
    self,
    slack::{self, home::PublishVerdict},
};
use gpui::App;
use std::sync::Arc;

/// The paired person, `user_id` of team `team_id`, opened connection `id`'s
/// page: show it every pane as it is now. Idempotent, so it claims nothing in
/// the inbox; a repeat only refreshes.
pub(crate) fn opened(id: &str, user_id: &str, team_id: &str, cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let Some((bot, scope)) = serving_person(id, user_id, team_id, cx) else {
        return;
    };
    cx.global_mut::<RemoteChannels>().home.queue.opened(&scope);
    publish_latest(scope, Some(bot), cx);
}

/// A pane's status or name changed: bring the pages the person is viewing up
/// to date. Reported from inside a workspace's update, while listing the
/// panes updates every workspace, so the rebuild waits for the update's end.
pub(crate) fn changed(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>()
        || !cx.global_mut::<RemoteChannels>().home.schedule_rebuild()
    {
        return;
    }
    cx.defer(rebuild);
}

/// The retry tick: forget pages no connection can serve any more, then send
/// boards that wait on a passing failure or on their connection.
pub(crate) fn retry(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let channels = cx.global_mut::<RemoteChannels>();
    let alive = channels.scope_alive();
    channels.home.queue.keep_scopes(alive);
    let ready = channels.home.queue.ready(true);
    send(ready, cx);
}

/// Build every viewed page anew.
fn rebuild(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let home = &mut cx.global_mut::<RemoteChannels>().home;
    home.start_rebuild();
    let scopes: Vec<Scope> = home.queue.viewing().cloned().collect();
    for scope in scopes {
        // A connection that cannot send now still gets its board; it waits
        // for the retry tick, and its thread links for a later rebuild.
        let bot = serving(&scope, cx).map(|(bot, _)| bot);
        publish_latest(scope, bot, cx);
    }
}

/// Ask `scope`'s page to show the panes as they are. Thread links not known
/// yet are asked of `bot` first, so the board carries their buttons.
fn publish_latest(scope: Scope, bot: Option<Arc<BotSession>>, cx: &mut App) {
    let board = linked_board(&scope, cx);
    let mut unlinked: Vec<String> = board.unlinked().map(str::to_owned).collect();
    unlinked.sort();
    unlinked.dedup();
    let Some(bot) = bot.filter(|_| !unlinked.is_empty()) else {
        want(&scope, board, cx);
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
                        let link = slack::home::permalink(&credentials, &channel, &root);
                        (root, link)
                    })
                    .collect::<Vec<_>>()
            })
            .await;
        cx.update(|cx| {
            let links = &mut cx.global_mut::<RemoteChannels>().home.links;
            for (root, link) in fetched {
                match link {
                    Ok(link) => {
                        links.insert((scope.connection.clone(), root), link);
                    }
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
            let board = linked_board(&scope, cx);
            want(&scope, board, cx);
        });
    })
    .detach();
}

/// The board for `scope`'s page, with the thread links known so far.
fn linked_board(scope: &Scope, cx: &mut App) -> Board {
    let mut board = board(scope, cx);
    let home = &cx.global::<RemoteChannels>().home;
    board.link(|root| {
        home.links
            .get(&(scope.connection.clone(), root.to_owned()))
            .cloned()
    });
    board
}

fn want(scope: &Scope, board: Board, cx: &mut App) {
    cx.global_mut::<RemoteChannels>()
        .home
        .queue
        .want(scope, board);
    flush(cx);
}

/// The connection's live bot and its thread scope, when `user_id` of
/// `team_id` is its paired person and its page can be published to now.
fn serving_person(
    id: &str,
    user_id: &str,
    team_id: &str,
    cx: &App,
) -> Option<(Arc<BotSession>, Scope)> {
    let connection = cx.global::<RemoteChannels>().live(id, cx)?;
    if !transport::app_home(connection.config.kind)
        || !connection.can_send()
        || !connection.config.accepts_person(user_id, team_id)
    {
        return None;
    }
    let bot = connection.bot.clone()?;
    let scope = connection.thread_scope(&bot)?;
    Some((bot, scope))
}

/// Every pane `/list` shows, with its status and its thread under `scope`.
fn board(scope: &Scope, cx: &mut App) -> Board {
    let listing = crate::control::exec::listing(cx);
    let threads = &cx.global::<RemoteChannels>().threads;
    let rows = crate::remote_channel::command::flatten(&listing)
        .iter()
        .map(|row| PaneRow {
            name: PaneRow::named(row),
            shown: threads.activity_of(row.summary.target),
            thread: threads.root_of_pane(scope, row.summary.target),
            link: None,
        })
        .collect();
    Board::new(rows, slack::home::ROW_CAPACITY)
}

/// Publish every page that is due now.
fn flush(cx: &mut App) {
    let ready = cx.global_mut::<RemoteChannels>().home.queue.ready(false);
    send(ready, cx);
}

fn send(ready: Vec<Publication>, cx: &mut App) {
    for publication in ready {
        cx.spawn(async move |cx| {
            let serving = cx.update(|cx| serving(&publication.scope, cx));
            let Some((bot, user_id)) = serving else {
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
                    slack::home::publish(&bot.credentials, &user_id, &look.view())
                })
                .await;
            cx.update(|cx| settle(&publication, &verdict, cx));
        })
        .detach();
    }
}

/// The session that may publish the page made under `scope`, and the person
/// whose page it is: the connection's live bot, if it is still that identity.
fn serving(scope: &Scope, cx: &App) -> Option<(Arc<BotSession>, String)> {
    let connection = cx.global::<RemoteChannels>().live(&scope.connection, cx)?;
    let bot = connection.bot.clone()?;
    let user_id = connection.config.recipient.as_ref()?.user_id.clone();
    (transport::app_home(connection.config.kind)
        && connection.can_send()
        && connection.thread_scope(&bot).as_ref() == Some(scope))
    .then_some((bot, user_id))
}

fn settle(publication: &Publication, verdict: &PublishVerdict, cx: &mut App) {
    let queue = &mut cx.global_mut::<RemoteChannels>().home.queue;
    match (queue.finish(publication, verdict), verdict) {
        (Settled::Published, _) => flush(cx),
        (Settled::Unavailable { first: true }, _) => crate::remote_channel::log_error(
            "Remote home page is off",
            &"Turn on the Home tab in the Slack app's App Home settings",
            "remote.home.disabled",
        ),
        // Not retried here: a channel that keeps failing would loop. The
        // retry tick or the person's next opening sends the latest board.
        (Settled::Retry, PublishVerdict::Transient(reason)) => crate::remote_channel::log_error(
            "Remote home page update will be retried",
            reason,
            "remote.home.retry",
        ),
        (Settled::Dropped, PublishVerdict::Refused(reason)) => crate::remote_channel::log_error(
            "Remote home page refused an update",
            reason,
            "remote.home.refused",
        ),
        _ => {}
    }
}
