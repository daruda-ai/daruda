//! Drives the pages `publish_queue` owes. This part takes the person opening
//! a page and the panes' changes, and lists the panes for the viewed pages;
//! `tap` acts on a page's buttons, `links` asks for thread links before a
//! board goes out, and `publish` sends boards off the main thread and
//! settles what that came to.

mod links;
mod publish;
mod tap;

pub(crate) use tap::tapped;

use super::board::{Board, PaneRow};
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{self, AppHome};
use gpui::App;
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
    links::publish_latest(scope, home, Some(bot), cx);
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
    publish::flush_retrying(cx);
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
        let bot = publish::serving(&scope, cx).map(|(bot, ..)| bot);
        links::publish_latest(scope, home, bot, cx);
    }
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

/// Every pane `/list` shows, with its status, its thread under `scope`, the
/// links known so far and whether commands address it, under what the last
/// tap came to.
fn board(scope: &Scope, capacity: usize, cx: &mut App) -> Board {
    let listing = crate::control::exec::listing(cx);
    let channels = cx.global_mut::<RemoteChannels>();
    let selected = channels
        .connections
        .get(&scope.connection)
        .and_then(|connection| connection.core.command_state().selected());
    let threads = &channels.threads;
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
    channels.home.board(scope, rows, capacity)
}
