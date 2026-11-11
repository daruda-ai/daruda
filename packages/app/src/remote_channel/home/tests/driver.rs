use super::super::Home;
use super::*;
use crate::remote_channel::global::RemoteChannels;
use crate::remote_channel::threads::status::PaneActivity;

/// Changes reported in one update make one rebuild, and only while the
/// person views a page.
#[test]
fn changes_schedule_one_rebuild_while_a_page_is_viewed() {
    let mut home = Home::default();
    assert!(!home.schedule_rebuild(), "no page viewed");
    home.queue.opened(&scope("work", "bot"));
    assert!(home.schedule_rebuild(), "the first change schedules");
    assert!(
        !home.schedule_rebuild(),
        "a second waits for the same rebuild"
    );
    home.start_rebuild();
    assert!(home.schedule_rebuild(), "a change after it schedules anew");
}

/// Status changes reported inside one update rebuild the viewed page once
/// that update ends, never during it; a connection that cannot send keeps
/// the board for the retry tick.
#[gpui::test]
fn reported_changes_rebuild_a_viewed_page_after_the_update(cx: &mut gpui::TestAppContext) {
    let scope = cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let channels = cx.global_mut::<RemoteChannels>();
        let connection = channels.connections.get_mut("work").unwrap();
        let scope = connection
            .thread_scope(connection.bot.as_ref().unwrap())
            .unwrap();
        // No bot: nothing reaches the network, and the board is held.
        connection.bot = None;
        channels.home.queue.opened(&scope);
        scope
    });
    cx.update(|cx| {
        for pane in 1..=3 {
            let pane = crate::remote_channel::bridge::PaneRef {
                workspace: daruda_store::project::WorkspaceUuid::new(),
                pane,
            };
            crate::remote_channel::panes::activity(pane, PaneActivity::Working, cx);
            assert!(
                cx.global::<RemoteChannels>().home.rebuild_pending,
                "scheduled, not built inside the update"
            );
        }
    });
    cx.run_until_parked();
    let home = &mut cx.update(|cx| {
        let home = &mut cx.global_mut::<RemoteChannels>().home;
        assert!(!home.rebuild_pending, "the rebuild ran");
        home.queue.ready(true)
    });
    assert_eq!(home.len(), 1, "the rebuilt board waits for the retry tick");
    assert_eq!(home[0].scope, scope);
}

/// A tap that names a pane no longer open, or a board the page no longer
/// shows, is answered on the page: the gone pane in `/stop`'s words, the
/// old board by showing the panes as they are now.
#[gpui::test]
fn a_tap_on_a_gone_pane_or_an_old_board_says_so_on_the_page(cx: &mut gpui::TestAppContext) {
    use super::super::token::{Act, Tap};
    use crate::surface::strings as s;
    let scope = cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let connection = &cx.global::<RemoteChannels>().connections["work"];
        let scope = connection
            .thread_scope(connection.bot.as_ref().unwrap())
            .unwrap();
        cx.global_mut::<RemoteChannels>().home.queue.opened(&scope);
        scope
    });
    // The page shows a pane closed since: no window holds it. The
    // generation its buttons carry.
    let show_gone = |cx: &mut gpui::TestAppContext| {
        cx.update(|cx| {
            let home = &mut cx.global_mut::<RemoteChannels>().home;
            let mut board = Board::new(vec![row("gone", None)], 10);
            board.number(&mut home.numbering);
            let generation = board.generation;
            home.queue.want(&scope, board);
            generation
        })
    };
    let gone = s::control::error_target_gone();
    let stale = s::remote_channel::home_stale();
    // The act tapped, and how many boards before the shown one it was made
    // for; no act is a value no page button carries.
    let cases = [
        (Some(Act::Stop), 0, &gone, "stop a closed pane"),
        (Some(Act::Select), 0, &gone, "select a closed pane"),
        (Some(Act::Stop), 1, &stale, "an old board"),
        (None, 0, &stale, "no page button's value"),
    ];
    for (act, older, want, label) in cases {
        let generation = show_gone(cx).wrapping_sub(older);
        let data = act.map_or_else(
            || "thr:stop:1.1".to_owned(),
            |act| {
                Tap {
                    generation,
                    index: 0,
                    act,
                }
                .token()
            },
        );
        cx.update(|cx| {
            cx.spawn(async move |cx| {
                super::super::driver::tapped("work", "1", "3", &data, cx).await
            })
            .detach()
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let home = &cx.global::<RemoteChannels>().home;
            let outcome = home.outcome_for_test(&scope).unwrap_or_default();
            assert!(outcome.contains(want.as_str()), "{label}: {outcome}");
            let board = home.queue.board(&scope).expect("a board is owed");
            assert!(board.rows.is_empty(), "{label}: rebuilt as the panes are");
            assert_eq!(
                board.outcome.as_ref().map(|outcome| &outcome.text),
                Some(&outcome),
                "{label}: the rebuilt page shows the result"
            );
        });
    }
}
