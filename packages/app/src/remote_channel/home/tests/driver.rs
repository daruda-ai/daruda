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
/// that update ends, never during it; a connection that cannot send holds
/// the board, and the retry tick builds the page anew.
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
    cx.update(|cx| {
        let home = &mut cx.global_mut::<RemoteChannels>().home;
        assert!(!home.rebuild_pending, "the rebuild ran");
        assert!(home.queue.board(&scope).is_some(), "a board is owed");
        assert!(home.queue.ready().is_empty(), "held, not sent");
        assert!(home.queue.retry(), "the retry tick builds it anew");
    });
}

/// The defect this guards: a disabled or unpaired connection was taken for
/// one reconnecting, so its page was kept and built anew on every retry
/// tick, only to wait again. Only a connection that will serve again keeps
/// its page.
#[gpui::test]
fn the_retry_tick_keeps_a_page_only_for_a_connection_that_serves_again(
    cx: &mut gpui::TestAppContext,
) {
    use crate::remote_channel::global::tests::{Lapse, lapse};
    for how in [Lapse::Reconnecting, Lapse::Disabled, Lapse::Unpaired] {
        cx.update(|cx| {
            let _outgoing = crate::remote_channel::global::tests::setup(cx);
            let connection = &cx.global::<RemoteChannels>().connections["work"];
            let scope = connection
                .thread_scope(connection.bot.as_ref().unwrap())
                .unwrap();
            cx.global_mut::<RemoteChannels>().home.queue.opened(&scope);
            lapse("work", how, cx);
            super::super::driver::retry(cx);
            let home = &cx.global::<RemoteChannels>().home;
            assert_eq!(home.queue.viewer(&scope).is_some(), how.kept(), "{how:?}");
        });
        cx.run_until_parked();
    }
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
            let mut board = Board::new(vec![row("gone", None)], 10, None);
            board.number(&mut home.numbering);
            let generation = board.generation();
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
            let connection = &cx.global::<RemoteChannels>().connections["work"];
            let generation = connection.generation;
            let bot = connection.bot.clone().unwrap();
            cx.spawn(async move |cx| {
                super::super::driver::tapped("work", generation, &bot, "1", "3", &data, cx).await
            })
            .detach()
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let home = &cx.global::<RemoteChannels>().home;
            let outcome = home.outcome_for_test(&scope).unwrap_or_default();
            assert!(outcome.contains(want.as_str()), "{label}: {outcome}");
            let board = home.queue.board(&scope).expect("a board is owed");
            assert!(board.rows().is_empty(), "{label}: rebuilt as the panes are");
            assert_eq!(
                board.outcome().map(|outcome| &outcome.text),
                Some(&outcome),
                "{label}: the rebuilt page shows the result"
            );
        });
    }
}

/// Thread links are asked off the main thread while the board goes out
/// without them. Each that lands is kept and builds the page again; one that
/// fails leaves its row without a button and is not asked again until the
/// retry tick.
#[gpui::test]
fn thread_links_land_after_the_board_and_build_the_page_again(cx: &mut gpui::TestAppContext) {
    use crate::remote_channel::transport::{AppHome, Credentials, TransportError};
    fn link(
        _: &Credentials,
        _: &str,
        root: &str,
    ) -> crate::remote_channel::transport::Result<String> {
        match root {
            "1.1" => Ok("https://x/p1".into()),
            _ => Err(TransportError::Message("no permalink".into())),
        }
    }
    let _stub = super::super::driver::stub_for_test(AppHome {
        link,
        ..crate::remote_channel::transport::app_home(daruda_config::remote::ChannelKind::Slack)
            .unwrap()
    });
    let scope = cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let connection = &cx.global::<RemoteChannels>().connections["work"];
        let scope = connection
            .thread_scope(connection.bot.as_ref().unwrap())
            .unwrap();
        cx.global_mut::<RemoteChannels>().home.queue.opened(&scope);
        scope
    });
    let threaded = |name: &str, root: &str| {
        let mut row = row(name, None);
        row.thread = Some(root.into());
        row
    };
    let rows = vec![threaded("a", "1.1"), threaded("b", "2.2")];
    let linked = |cx: &mut gpui::TestAppContext| {
        let rows = rows.clone();
        cx.update(|cx| {
            let board = cx
                .global_mut::<RemoteChannels>()
                .home
                .board(&scope, rows, 10);
            let links: Vec<Option<String>> =
                board.rows().iter().map(|row| row.link.clone()).collect();
            (board, links)
        })
    };
    let (board, links) = linked(cx);
    assert_eq!(links, [None, None], "nothing known yet");
    cx.update(|cx| super::super::driver::ask_links_for_test(&scope, &board, cx));
    assert!(
        cx.update(|cx| cx
            .global_mut::<RemoteChannels>()
            .home
            .queue
            .ask_links(&scope, ["1.1", "2.2"])
            .is_empty()),
        "asked once, pending meanwhile"
    );
    cx.run_until_parked();
    let (_, links) = linked(cx);
    assert_eq!(links, [Some("https://x/p1".to_owned()), None]);
    assert!(
        cx.update(|cx| cx
            .global_mut::<RemoteChannels>()
            .home
            .queue
            .ask_links(&scope, ["2.2"])
            .is_empty()),
        "a failed link is not asked again before the retry tick"
    );
    cx.update(super::super::driver::retry);
    cx.update(|cx| {
        assert_eq!(
            cx.global_mut::<RemoteChannels>()
                .home
                .queue
                .ask_links(&scope, ["1.1", "2.2"]),
            ["2.2"],
            "the retry tick asks a failed link again"
        )
    });
}
