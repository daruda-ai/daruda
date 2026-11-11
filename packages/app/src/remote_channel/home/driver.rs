//! Drives the pages `publish_queue` owes: takes the person opening a page
//! and the panes' changes, lists the panes with their status and thread
//! links, publishes off the main thread, and settles what that came to.

use super::board::{Board, Outcome, PaneRow};
use super::look::HomeLook;
use super::publish_queue::{Publication, Settled};
use super::token::{Act, Tap};
use crate::remote_channel::dispatch::{self, Effect};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{self, AppHome, PageVerdict};
use crate::surface::strings as s;
use gpui::{App, AsyncApp};
use std::sync::Arc;

/// The paired person, `user_id` of team `team_id`, opened connection `id`'s
/// page: show it every pane as it is now. Idempotent, so it claims nothing in
/// the inbox; a repeat only refreshes.
pub(crate) fn opened(id: &str, user_id: &str, team_id: &str, cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let Some((bot, home, scope)) = serving_person(id, user_id, team_id, cx) else {
        return;
    };
    cx.global_mut::<RemoteChannels>().home.queue.opened(&scope);
    publish_latest(scope, home, Some(bot), cx);
}

/// The paired person tapped the button carrying `data` on connection `id`'s
/// page, and the tap is claimed. Select or stop the pane it names, as `/use`
/// and `/stop` would, and show what that came to on the page itself: the
/// conversation is not where the person is looking.
pub(crate) async fn tapped(id: &str, user_id: &str, team_id: &str, data: &str, cx: &mut AsyncApp) {
    let Some((scope, acted)) = cx.update(|cx| act(id, user_id, team_id, data, cx)) else {
        return;
    };
    let text = match acted {
        Acted::Said(text) => text,
        Acted::Stopping(effect) => said(effect.settled().await),
    };
    cx.update(|cx| {
        // The connection may have gone, or changed identity, while the stop
        // settled; a page made under the old identity is not shown again.
        let Some((bot, home, now)) = serving_person(id, user_id, team_id, cx) else {
            return;
        };
        if now != scope {
            return;
        }
        let outcome = text.map(|text| Outcome {
            at: chrono::Utc::now().timestamp(),
            text,
        });
        cx.global_mut::<RemoteChannels>()
            .home
            .queue
            .tapped(&scope, outcome);
        publish_latest(scope, home, Some(bot), cx);
    });
}

/// What a tap did at once, or the stop it started.
enum Acted {
    Said(Option<String>),
    Stopping(Effect),
}

fn act(id: &str, user_id: &str, team_id: &str, data: &str, cx: &mut App) -> Option<(Scope, Acted)> {
    if !cx.has_global::<RemoteChannels>() {
        return None;
    }
    let (_, _, scope) = serving_person(id, user_id, team_id, cx)?;
    let tapped = Tap::parse(data).and_then(|tap| {
        let queue = &cx.global::<RemoteChannels>().home.queue;
        Some((queue.resolve(&scope, tap)?, tap.act))
    });
    let target = dispatch::Target::Remote(id.into());
    let acted = match tapped {
        // The page is rebuilt below, so the person sees what is there now.
        None => Acted::Said(Some(s::remote_channel::home_stale())),
        Some((pane, Act::Select)) => Acted::Said(dispatch::select_live(pane, &target, cx)),
        Some((pane, Act::Stop)) => Acted::Stopping(dispatch::stop_pane(pane, &target, cx)),
    };
    Some((scope, acted))
}

/// The words a settled effect has for the person, if any.
fn said(effect: Effect) -> Option<String> {
    match effect {
        Effect::Reply(reply) => Some(reply.text),
        Effect::Feedback { label, .. } => Some(label),
        Effect::None | Effect::Later(_) => None,
    }
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
        // Only a removed connection has no page left; the retry tick
        // forgets it.
        let Some(home) = page_of(&scope, cx) else {
            continue;
        };
        // A connection that cannot send now still gets its board; it waits
        // for the retry tick, and its thread links for a later rebuild.
        let bot = serving(&scope, cx).map(|(bot, ..)| bot);
        publish_latest(scope, home, bot, cx);
    }
}

/// Ask `scope`'s page to show the panes as they are. Thread links not known
/// yet are asked of `bot` first, so the board carries their buttons.
fn publish_latest(scope: Scope, home: AppHome, bot: Option<Arc<BotSession>>, cx: &mut App) {
    let board = linked_board(&scope, home, cx);
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
                        let link = (home.link)(&credentials, &channel, &root);
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
            let board = linked_board(&scope, home, cx);
            want(&scope, board, cx);
        });
    })
    .detach();
}

/// The board for `scope`'s page, with the thread links known so far.
fn linked_board(scope: &Scope, home: AppHome, cx: &mut App) -> Board {
    let mut board = board(scope, home.capacity, cx);
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

/// The connection's live bot, its channel's app home and its thread scope,
/// when `user_id` of `team_id` is its paired person and its page can be
/// published to now.
fn serving_person(
    id: &str,
    user_id: &str,
    team_id: &str,
    cx: &App,
) -> Option<(Arc<BotSession>, AppHome, Scope)> {
    let connection = cx.global::<RemoteChannels>().live(id, cx)?;
    let home = transport::app_home(connection.config.kind)?;
    if !connection.can_send() || !connection.config.accepts_person(user_id, team_id) {
        return None;
    }
    let bot = connection.bot.clone()?;
    let scope = connection.thread_scope(&bot)?;
    Some((bot, home, scope))
}

/// The app home of the channel `scope`'s connection is on, while the
/// connection exists.
fn page_of(scope: &Scope, cx: &App) -> Option<AppHome> {
    let connection = cx
        .global::<RemoteChannels>()
        .connections
        .get(&scope.connection)?;
    transport::app_home(connection.config.kind)
}

/// Every pane `/list` shows, with its status, its thread under `scope` and
/// whether commands address it, under what the last tap came to.
fn board(scope: &Scope, capacity: usize, cx: &mut App) -> Board {
    let listing = crate::control::exec::listing(cx);
    let channels = cx.global::<RemoteChannels>();
    let threads = &channels.threads;
    let selected = channels
        .connections
        .get(&scope.connection)
        .and_then(|connection| connection.core.command_state().selected());
    let rows = crate::remote_channel::command::flatten(&listing)
        .iter()
        .map(|row| PaneRow {
            pane: row.summary.target,
            name: PaneRow::named(row),
            shown: threads.activity_of(row.summary.target),
            thread: threads.root_of_pane(scope, row.summary.target),
            link: None,
            selected: selected == Some(row.summary.target),
        })
        .collect();
    let mut board = Board::new(rows, capacity);
    let home = &mut cx.global_mut::<RemoteChannels>().home;
    board.outcome = home.queue.outcome(scope).cloned();
    board.number(&mut home.numbering);
    board
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

/// The session that may publish the page made under `scope`, the channel's
/// app home it writes to, and the person whose page it is: the connection's
/// live bot, if it is still that identity.
fn serving(scope: &Scope, cx: &App) -> Option<(Arc<BotSession>, AppHome, String)> {
    let connection = cx.global::<RemoteChannels>().live(&scope.connection, cx)?;
    let home = transport::app_home(connection.config.kind)?;
    let bot = connection.bot.clone()?;
    let user_id = connection.config.recipient.as_ref()?.user_id.clone();
    (connection.can_send() && connection.thread_scope(&bot).as_ref() == Some(scope))
        .then_some((bot, home, user_id))
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
        // retry tick or the person's next opening sends the latest board.
        (Settled::Retry, PageVerdict::Transient(reason)) => crate::remote_channel::log_error(
            "Remote home page update will be retried",
            reason,
            "remote.home.retry",
        ),
        (Settled::Dropped, PageVerdict::Refused(reason)) => crate::remote_channel::log_error(
            "Remote home page refused an update",
            reason,
            "remote.home.refused",
        ),
        _ => {}
    }
}
