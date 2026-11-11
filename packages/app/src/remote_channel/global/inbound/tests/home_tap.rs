//! A tap on the app home: claimed once, acting on the pane it names, and
//! answered on the page rather than in the conversation.

use super::*;

/// A stop tap reaches its pane and its result heads the page; the same tap
/// delivered again acts no more, while a new tap on the same button does.
#[gpui::test]
async fn a_home_tap_acts_once_and_answers_on_the_page(cx: &mut TestAppContext) {
    use crate::surface::strings as s;
    cx.executor().allow_parking();
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let pane = fixture.pane_ref(cx);
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let (generation, scope) = cx.update(|cx| {
        let generation = install_connection("work", bot.clone(), cx);
        crate::remote_channel::home::driver::opened("work", "user", "scope", cx);
        let scope = cx.global::<RemoteChannels>().connections["work"]
            .thread_scope(&bot)
            .unwrap();
        (generation, scope)
    });
    cx.run_until_parked();
    let select = cx
        .update(|cx| {
            cx.global::<RemoteChannels>()
                .home
                .button_for_test(&scope, pane, false)
        })
        .expect("every pane can be selected");
    let selected = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.global_mut::<RemoteChannels>()
                .connections
                .get_mut("work")
                .unwrap()
                .core
                .command_state()
                .selected()
        })
    };

    tap(generation, "select-1", &select, cx).await;
    assert_eq!(selected(cx), Some(pane), "the tap selects its pane");
    let said = cx.update(|cx| {
        cx.global::<RemoteChannels>()
            .home
            .outcome_for_test(&scope)
            .unwrap()
    });
    assert!(
        said.starts_with(&s::control::selected("")),
        "the selection heads the page: {said}"
    );
    cx.update(|cx| {
        cx.global_mut::<RemoteChannels>()
            .connections
            .get_mut("work")
            .unwrap()
            .core
            .command_state_mut()
            .select(None)
    });
    tap(generation, "select-1", &select, cx).await;
    assert_eq!(selected(cx), None, "a redelivered tap is claimed already");

    fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_working_for_test(fixture.pane(), cx)
    });
    cx.update(|cx| {
        crate::remote_channel::panes::activity(
            pane,
            crate::remote_channel::threads::status::PaneActivity::Working,
            cx,
        )
    });
    cx.run_until_parked();
    let stop = cx
        .update(|cx| {
            cx.global::<RemoteChannels>()
                .home
                .button_for_test(&scope, pane, true)
        })
        .expect("a working pane can be stopped");
    tap(generation, "stop-1", &stop, cx).await;
    let said = cx.update(|cx| {
        cx.global::<RemoteChannels>()
            .home
            .outcome_for_test(&scope)
            .unwrap()
    });
    assert_eq!(said, s::control::stopped(), "the result heads the page");
    let stop_now = |cx: &mut TestAppContext| {
        fixture
            .workspace
            .update(cx, |ws, cx| ws.control_stop(fixture.pane(), cx))
    };
    assert_eq!(
        stop_now(cx),
        Ok(crate::control::result::StopDisposition::AlreadyIdle),
        "the tap stopped the working pane"
    );
    fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_working_for_test(fixture.pane(), cx)
    });
    tap(generation, "stop-1", &stop, cx).await;
    assert_eq!(
        stop_now(cx),
        Ok(crate::control::result::StopDisposition::Stopped),
        "the redelivered stop did not stop it again"
    );
    fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_working_for_test(fixture.pane(), cx)
    });
    tap(generation, "stop-2", &stop, cx).await;
    assert_eq!(
        stop_now(cx),
        Ok(crate::control::result::StopDisposition::AlreadyIdle),
        "a new tap on the same button stops it again"
    );
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(bot);
    wait_release(root.path());
}

/// Serve a tap on "work"'s page as the connection's queue does, claim first.
async fn tap(generation: uuid::Uuid, event_id: &str, data: &str, cx: &mut TestAppContext) {
    let event = transport::HomeEvent {
        event_id: event_id.into(),
        user_id: "user".into(),
        team_id: "scope".into(),
        kind: transport::HomeEventKind::Tap { data: data.into() },
    };
    let mut app = cx.to_async();
    let served = serve_home_tap(
        "work",
        generation,
        event,
        &crate::runs::RunsHost::claim_inbound,
        &mut app,
    )
    .await;
    assert!(matches!(served, Served::Done), "the claim is durable");
    cx.run_until_parked();
}
