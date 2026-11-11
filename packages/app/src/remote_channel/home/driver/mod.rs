//! Drives the pages `publish_queue` owes. This part takes the person opening
//! a page and the panes' changes, and builds the viewed pages anew; `tap`
//! acts on a page's buttons, `links` asks for thread links, and `publish`
//! sends boards off the main thread and settles what that came to.

mod links;
mod publish;
mod tap;

pub(crate) use tap::tapped;

use super::board::{Board, PaneRow};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::command::ListingRow;
use crate::remote_channel::global::{Connection, RemoteChannels};
use crate::remote_channel::runtime::BotSession;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::{self, AppHome};
use daruda_config::remote::ChannelKind;
use gpui::App;

/// The paired person, `user_id` of team `team_id`, opened connection `id`'s
/// page: show it every pane as it is now. Idempotent, so it claims nothing in
/// the inbox; a repeat only refreshes. Taken while the connection reconnects
/// too: the board waits for it to send again.
pub(crate) fn opened(id: &str, user_id: &str, team_id: &str, cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let channels = cx.global::<RemoteChannels>();
    // Without a bot the connection has no identity to keep a page under;
    // the person's next opening, once it has one, shows the page.
    let Some(scope) = channels.live(id, cx).and_then(|connection| {
        let bot = connection.bot.as_ref()?;
        person_scope(connection, bot, user_id, team_id)
    }) else {
        return;
    };
    cx.global_mut::<RemoteChannels>().home.queue.opened(&scope);
    changed(cx);
}

/// What the person views changed: a pane's status, name or selection, a
/// pane opened, a tap's result, or a thread link learned. Bring the viewed
/// pages up to date. Often reported from inside a workspace's update, while
/// listing the panes updates every workspace, so the rebuild waits for the
/// update's end.
pub(crate) fn changed(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>()
        || !cx.global_mut::<RemoteChannels>().home.schedule_rebuild()
    {
        return;
    }
    cx.defer(rebuild);
}

/// The retry tick: forget pages no connection can serve any more, then
/// build anew the pages whose board waits on a passing failure or on their
/// connection, once that connection can send. The panes have moved on since,
/// so the board that waited is not sent as it was.
pub(crate) fn retry(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let channels = cx.global::<RemoteChannels>();
    let (alive, sendable) = (channels.scope_alive(cx), channels.scope_sendable(cx));
    let channels = cx.global_mut::<RemoteChannels>();
    channels.home.queue.keep_scopes(alive);
    if channels.home.queue.retry(sendable) {
        changed(cx);
    }
}

/// Build every viewed page anew.
fn rebuild(cx: &mut App) {
    if !cx.has_global::<RemoteChannels>() {
        return;
    }
    let home = &mut cx.global_mut::<RemoteChannels>().home;
    home.start_rebuild();
    let scopes: Vec<Scope> = home.queue.viewing().cloned().collect();
    if scopes.is_empty() {
        return;
    }
    // One listing for every page: it walks, and updates, every workspace.
    let listing = crate::remote_channel::command::flatten(&crate::control::exec::listing(cx));
    for scope in scopes {
        // Only a removed connection has no page left; the retry tick
        // forgets it.
        let Some(home) = page_of(&scope, cx) else {
            continue;
        };
        let board = board(&scope, home.capacity, &listing, cx);
        // A connection that cannot send now still gets its board; it waits
        // for a retry tick the connection can send on. Its thread links are
        // asked on a later rebuild.
        if let Some((bot, ..)) = publish::serving(&scope, cx) {
            links::ask(&scope, home, bot, &board, cx);
        }
        publish::want(&scope, board, cx);
    }
}

/// The scope connection `id`'s page is kept under while `bot` serves it,
/// when its channel has a page and `user_id` of `team_id` is its paired
/// person. Asks only who the page is for, not whether it can be sent now.
fn person_scope(
    connection: &Connection,
    bot: &BotSession,
    user_id: &str,
    team_id: &str,
) -> Option<Scope> {
    app_home(connection.config.kind)?;
    if !connection.config.accepts_person(user_id, team_id) {
        return None;
    }
    connection.thread_scope(bot)
}

/// Whether what was made under `scope` may still be shown: its connection
/// is live and serves that identity, or is reconnecting and has not said
/// which identity yet.
fn identity_holds(scope: &Scope, cx: &App) -> bool {
    let channels = cx.global::<RemoteChannels>();
    channels.live(&scope.connection, cx).is_some() && (channels.scope_alive(cx))(scope)
}

/// The app home of the channel `scope`'s connection is on, while the
/// connection exists.
fn page_of(scope: &Scope, cx: &App) -> Option<AppHome> {
    let connection = cx
        .global::<RemoteChannels>()
        .connections
        .get(&scope.connection)?;
    app_home(connection.config.kind)
}

/// The app home a channel of `kind` has. Tests stand a stub in for the
/// network on the channels that have one.
fn app_home(kind: ChannelKind) -> Option<AppHome> {
    let home = transport::app_home(kind)?;
    #[cfg(test)]
    if let Some(stub) = STUB.get() {
        return Some(stub);
    }
    Some(home)
}

/// Every pane `/list` shows, with its status, its thread under `scope` and
/// whether commands address it, under what the last tap came to.
fn board(scope: &Scope, capacity: usize, listing: &[ListingRow], cx: &mut App) -> Board {
    let channels = cx.global_mut::<RemoteChannels>();
    let selected: Option<PaneRef> = channels
        .connections
        .get(&scope.connection)
        .and_then(|connection| connection.core.command_state().selected());
    let threads = &channels.threads;
    let rows = listing
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

/// Ask the links `board` lacks for `scope`'s page, as a rebuild does.
#[cfg(test)]
pub(super) fn ask_links_for_test(scope: &Scope, board: &Board, cx: &mut App) {
    let (bot, home, _) = publish::serving(scope, cx).expect("the connection can send");
    links::ask(scope, home, bot, board, cx);
}

#[cfg(test)]
thread_local! {
    static STUB: std::cell::Cell<Option<AppHome>> = const { std::cell::Cell::new(None) };
}

/// Stand `home` in for the channel's app home on this thread until the
/// guard drops.
#[cfg(test)]
pub(crate) fn stub_for_test(home: AppHome) -> impl Drop {
    struct Unstub;
    impl Drop for Unstub {
        fn drop(&mut self) {
            STUB.set(None);
        }
    }
    STUB.set(Some(home));
    Unstub
}
