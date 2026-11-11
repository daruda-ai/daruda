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
