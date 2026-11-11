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

/// A tap claimed while its connection cannot send, as when a reopened bot
/// is back before the worker says it holds the claim again, still acts and
/// keeps its result. The page showing it waits, and goes out on the retry
/// tick once the connection sends again.
#[gpui::test]
async fn a_home_tap_acts_while_its_connection_cannot_send(cx: &mut TestAppContext) {
    use crate::remote_channel::runtime::Status;
    use crate::remote_channel::transport::{AppHome, Credentials, PageVerdict, PageView};
    use crate::surface::strings as s;
    fn shown(_: &Credentials, _: &str, _: &PageView<'_>) -> PageVerdict {
        PageVerdict::Published
    }
    cx.executor().allow_parking();
    let _stub = crate::remote_channel::home::driver::stub_for_test(AppHome {
        publish: shown,
        ..transport::app_home(daruda_config::remote::ChannelKind::Slack).unwrap()
    });
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
    let shown_outcome = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.global::<RemoteChannels>()
                .home
                .shown_outcome_for_test(&scope)
        })
    };
    assert_eq!(shown_outcome(cx), Some(None), "the page shows the panes");
    let select = cx
        .update(|cx| {
            cx.global::<RemoteChannels>()
                .home
                .button_for_test(&scope, pane, false)
        })
        .expect("every pane can be selected");
    let set_status = |status, cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.global_mut::<RemoteChannels>()
                .connections
                .get_mut("work")
                .unwrap()
                .status = status;
        })
    };
    set_status(Status::Failed, cx);

    tap(generation, "select-1", &select, cx).await;
    let selected = cx.update(|cx| {
        cx.global::<RemoteChannels>().connections["work"]
            .core
            .command_state()
            .selected()
    });
    assert_eq!(selected, Some(pane), "the tap acted");
    assert_eq!(
        shown_outcome(cx),
        Some(None),
        "the page waits for the connection"
    );

    set_status(Status::Connected, cx);
    cx.update(crate::remote_channel::home::driver::retry);
    cx.run_until_parked();
    let shown = shown_outcome(cx).flatten().unwrap_or_default();
    assert!(
        shown.starts_with(&s::control::selected("")),
        "the retry tick shows the tap's result: {shown}"
    );
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(bot);
    wait_release(root.path());
}

/// Serve a tap on "work"'s page as the connection's queue does, claim first.
async fn tap(generation: uuid::Uuid, event_id: &str, data: &str, cx: &mut TestAppContext) {
    let paired = Tapper {
        generation,
        user_id: "user",
        team_id: "scope",
    };
    tap_as(
        paired,
        event_id,
        data,
        &crate::runs::RunsHost::claim_inbound,
        cx,
    )
    .await;
}

/// Who a tap comes from: the person and team the event names, and the
/// worker that delivered it.
#[derive(Clone, Copy)]
struct Tapper<'a> {
    generation: uuid::Uuid,
    user_id: &'a str,
    team_id: &'a str,
}

/// [`tap`] from `tapper`, claimed through `claim`.
async fn tap_as<F: std::future::Future<Output = ClaimResult>>(
    tapper: Tapper<'_>,
    event_id: &str,
    data: &str,
    claim: &impl Fn(&InboxStore, ChannelKey, InboxId) -> F,
    cx: &mut TestAppContext,
) {
    let event = transport::HomeEvent {
        event_id: event_id.into(),
        user_id: tapper.user_id.into(),
        team_id: tapper.team_id.into(),
        kind: transport::HomeEventKind::Tap { data: data.into() },
    };
    let mut app = cx.to_async();
    let served = serve_home_tap("work", tapper.generation, event, claim, &mut app).await;
    assert!(matches!(served, Served::Done), "the claim is durable");
    cx.run_until_parked();
}

/// Only the paired person's tap, delivered by the worker and served by the
/// bot running now, acts. Another person's or team's tap is claimed as
/// their message would be, so a redelivery is not weighed again, and does
/// nothing; one from an old worker is not even claimed; one that reaches
/// the page after its bot was replaced does nothing either. None opens the
/// page or leaves a result on it.
#[gpui::test]
async fn a_home_tap_from_anyone_but_the_paired_person_does_nothing(cx: &mut TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
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
    let claimed = Rc::new(RefCell::new(Vec::<String>::new()));
    let record = {
        let claimed = claimed.clone();
        move |inbox: &InboxStore, channel, id: InboxId| {
            if let InboxId::Opaque(event) = &id {
                claimed.borrow_mut().push(event.clone());
            }
            crate::runs::RunsHost::claim_inbound(inbox, channel, id)
        }
    };
    let selected = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.global::<RemoteChannels>().connections["work"]
                .core
                .command_state()
                .selected()
        })
    };
    let outcome = |cx: &mut TestAppContext| {
        cx.update(|cx| cx.global::<RemoteChannels>().home.outcome_for_test(&scope))
    };
    let cases = [
        (
            "another person",
            Tapper {
                generation,
                user_id: "stranger",
                team_id: "scope",
            },
            true,
        ),
        (
            "another team",
            Tapper {
                generation,
                user_id: "user",
                team_id: "elsewhere",
            },
            true,
        ),
        (
            "an old worker",
            Tapper {
                generation: uuid::Uuid::new_v4(),
                user_id: "user",
                team_id: "scope",
            },
            false,
        ),
    ];
    for (label, tapper, claims) in cases {
        claimed.borrow_mut().clear();
        tap_as(tapper, label, &select, &record, cx).await;
        let want: &[&str] = if claims { &[label] } else { &[] };
        assert_eq!(*claimed.borrow(), want, "{label}: as a message is");
        assert_eq!(selected(cx), None, "{label}: nothing selected");
        assert_eq!(outcome(cx), None, "{label}: no result on the page");
    }

    // The same button, tapped by the paired person through the bot running
    // now, acts: the cases above are refused for who sent them.
    let tapped = |bot: Arc<BotSession>, cx: &mut TestAppContext| {
        let select = select.clone();
        let mut app = cx.to_async();
        async move {
            crate::remote_channel::home::driver::tapped(
                "work", generation, &bot, "user", "scope", &select, &mut app,
            )
            .await;
        }
    };
    tapped(bot.clone(), cx).await;
    cx.run_until_parked();
    assert_eq!(selected(cx), Some(pane), "the paired person's tap acts");
    let shown = outcome(cx);
    assert!(shown.is_some(), "and its result heads the page");
    cx.update(|cx| {
        RemoteChannels::select("work", None, cx);
    });

    // Claimed by the bot that served then, and acted on once another
    // serves: the claiming bot no longer runs the connection.
    cx.update(|cx| {
        cx.global_mut::<RemoteChannels>()
            .connections
            .get_mut("work")
            .unwrap()
            .publish_bot(crate::remote_channel::global::tests::ready_bot());
    });
    tapped(bot.clone(), cx).await;
    cx.run_until_parked();
    assert_eq!(selected(cx), None, "a replaced bot's tap selects nothing");
    assert_eq!(outcome(cx), shown, "and leaves no result on the page");

    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(bot);
    wait_release(root.path());
}

/// A tap whose claim failed is held, not dropped: the reopened inbox claims
/// it again ahead of the message queued behind it, and it is then served.
#[gpui::test]
async fn a_home_tap_whose_claim_failed_is_served_first_on_the_reopened_inbox(
    cx: &mut TestAppContext,
) {
    use crate::surface::strings as s;
    use std::{cell::RefCell, rc::Rc};

    cx.executor().allow_parking();
    let root = tempfile::tempdir().unwrap();
    let first = open_bot(root.path(), || Ok(InboxSeed::default()));
    let generation = cx.update(|cx| install_connection("work", first.clone(), cx));
    cx.update(|cx| run_worker("work", cx));
    let attempts = Rc::new(RefCell::new(Vec::<String>::new()));
    let recorded = attempts.clone();
    let (events, receiver) = futures::channel::mpsc::unbounded();
    cx.update(|cx| {
        serve_events(
            receiver,
            move |inbox: &InboxStore, channel, id| {
                let event = match &id {
                    InboxId::Opaque(event) => event.clone(),
                    InboxId::Monotonic(update) => update.to_string(),
                };
                let fail = recorded.borrow().is_empty();
                recorded.borrow_mut().push(event);
                if fail {
                    futures::future::Either::Left(std::future::ready(Err(CommitError::Io(
                        std::io::Error::other("temporary storage failure"),
                    ))))
                } else {
                    futures::future::Either::Right(inbox.claim(channel, id))
                }
            },
            cx,
        )
    });

    // A button no board carries: served, it says the page was stale.
    events
        .unbounded_send(WorkerEvent {
            id: "work".into(),
            generation,
            payload: WorkerPayload::Home(transport::HomeEvent {
                event_id: "tap".into(),
                user_id: "user".into(),
                team_id: "scope".into(),
                kind: transport::HomeEventKind::Tap {
                    data: "hom:0:0:stop".into(),
                },
            }),
        })
        .unwrap();
    events
        .unbounded_send(from("stranger", "after", generation))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *attempts.borrow(),
        ["tap"],
        "a later command must not be claimed ahead of a held tap"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(reopen_requested("work", cx));

    drop(first);
    let reopened = open_bot(root.path(), || panic!("recovery must not reimport"));
    // As the worker reports a reopened session: ready, then connecting, so
    // the page may be published to again.
    for payload in [
        WorkerPayload::Ready(reopened.clone()),
        WorkerPayload::Status(crate::remote_channel::runtime::Status::Connecting),
    ] {
        events
            .unbounded_send(WorkerEvent {
                id: "work".into(),
                generation,
                payload,
            })
            .unwrap();
    }
    // A real claim completes on the inbox's writer thread, outside the
    // executor, so parking alone does not mean the backlog has drained.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while attempts.borrow().len() < 3 && std::time::Instant::now() < deadline {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(*attempts.borrow(), ["tap", "tap", "after"]);
    let said = cx.update(|cx| {
        let channels = cx.global::<RemoteChannels>();
        let scope = channels.connections["work"]
            .thread_scope(&reopened)
            .unwrap();
        channels.home.outcome_for_test(&scope)
    });
    assert_eq!(
        said,
        Some(s::remote_channel::home_stale()),
        "the held tap was served once claimed"
    );
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(events);
    cx.run_until_parked();
    drop(reopened);
    wait_release(root.path());
}
