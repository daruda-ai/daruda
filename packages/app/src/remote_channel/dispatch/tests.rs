use super::{Aimed, Effect, Target, handle_in_app, target::aim_in_app};
use crate::remote_channel::bridge::{InboundAction, PaneRef, Routed, Unaimed};

/// [`handle_in_app`] from an async test.
pub(crate) fn handle(aimed: Aimed, target: &Target, cx: &mut gpui::AsyncApp) -> Effect {
    cx.update(|cx| handle_in_app(aimed, target, cx))
}

/// [`aim_in_app`] from an async test.
pub(crate) fn aim(routed: Routed, cx: &mut gpui::AsyncApp) -> Aimed {
    cx.update(|cx| aim_in_app(routed, cx))
}

/// A toggle's toast says what its card says; a refused text reply gets a
/// reply, while a taken one is told by the next card.
#[test]
fn question_answers_are_told_what_their_card_says() {
    use crate::remote_channel::bridge::{BotQuestionOutcome, QuestionButton, QuestionReply};
    use crate::surface::strings as s;
    let tap = QuestionReply::Button(QuestionButton::Option(0));
    let label = |effect| match effect {
        super::Effect::Feedback { label, .. } => Some(label),
        super::Effect::Reply(reply) => Some(reply.text),
        _ => None,
    };
    assert_eq!(
        label(super::question_effect(&tap, BotQuestionOutcome::Updated)),
        Some(s::notification::remote_question_updated())
    );
    assert_eq!(
        label(super::question_effect(&tap, BotQuestionOutcome::Next)),
        Some(s::notification::remote_question_next())
    );
    let text = QuestionReply::Text("teal".into());
    assert_eq!(
        label(super::question_effect(&text, BotQuestionOutcome::Next)),
        None
    );
    assert_eq!(
        label(super::question_effect(
            &text,
            BotQuestionOutcome::AlreadySettled
        )),
        Some(s::notification::remote_question_stale())
    );
}

/// A message that named no chat, on a bridge that remembered none, still
/// reaches an agent — `aim` lends it the app's own lane. The sender is told
/// which one, because from the phone a lent lane and the one they were
/// talking to look exactly alike, and only one of them is what they meant.
#[gpui::test]
async fn a_lent_target_is_reported_back_to_the_sender(cx: &mut gpui::TestAppContext) {
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let pane = fixture.pane();
    let expected = fixture
        .workspace
        .read_with(cx, |ws, cx| ws.control_chat_label(pane, cx))
        .expect("the fixture's pane has a label");
    let mut async_cx = cx.to_async();

    let aimed = aim(
        Routed::NeedsTarget(Unaimed::Text {
            text: "ship it".into(),
        }),
        &mut async_cx,
    );
    let effect = handle(aimed, &super::Target::Telegram, &mut async_cx);

    match effect {
        super::Effect::Reply(reply) => assert_eq!(
            reply.text,
            crate::surface::strings::control::target_lent(
                &expected.path,
                &expected.shown.agent_name
            ),
        ),
        _ => panic!("a lent target must be reported, not silently used"),
    }
}

/// The other half, and the reason the notice is worth having: a message
/// that named its own chat is delivered without a word. Told every time,
/// the notice would be chatter the sender learns to scroll past — which is
/// the one thing it cannot afford to become.
#[gpui::test]
async fn a_target_the_sender_named_is_delivered_in_silence(cx: &mut gpui::TestAppContext) {
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let pane = fixture.pane_ref(cx);
    let mut async_cx = cx.to_async();

    let effect = handle(
        super::Aimed {
            action: InboundAction::InjectPrompt {
                pane,
                text: "ship it".into(),
            },
            adopted: None,
        },
        &super::Target::Telegram,
        &mut async_cx,
    );

    assert!(
        matches!(effect, super::Effect::None),
        "a chat the sender picked needs no answer about where the message went"
    );
}

/// `/task` numbers the tasks, and `/task start <n>` starts the one at
/// that number. The start is accepted at once and recorded in the task
/// log before anything opens, so a second start is refused as soon as
/// that lands.
#[gpui::test]
async fn task_start_runs_the_task_the_listing_numbered(cx: &mut gpui::TestAppContext) {
    use crate::control::spec::{ControlCommand, Ordinal, TaskCommand};
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let created = fixture.workspace.update(cx, |ws, cx| {
        let lane = ws.control_active_lane();
        ws.control_task_create(
            lane.project,
            crate::test_support::new_task(
                "Here",
                "p",
                Some(crate::control::result::LaneHandle::new(ws.uuid(), lane)),
            ),
            cx,
        )
        .expect("created")
    });
    cx.run_until_parked();
    let task = created.await.expect("recorded");
    cx.update(|cx| crate::telegram::global::install_for_test(true, Some(42), cx));
    let mut async_cx = cx.to_async();
    let mut run = |command| {
        let aimed = super::Aimed {
            action: InboundAction::RunCommand { command },
            adopted: None,
        };
        match handle(aimed, &super::Target::Telegram, &mut async_cx) {
            super::Effect::Reply(reply) => reply.text,
            _ => panic!("a command is answered"),
        }
    };

    let listing = run(ControlCommand::Task(TaskCommand::List));
    let row = format!(
        "1. {}",
        crate::surface::strings::control::approval_quoted(
            daruda_control_types::agent_text::isolate("Here")
        )
    );
    assert!(listing.contains(&row), "{listing}");
    let started = run(ControlCommand::Task(TaskCommand::Start(Ordinal(1))));
    assert_eq!(
        started,
        crate::surface::strings::control::task_starting(
            crate::surface::strings::control::approval_quoted(
                daruda_control_types::agent_text::isolate("Here")
            )
        )
    );
    cx.run_until_parked();
    let mut async_cx = cx.to_async();
    let mut run = |command| {
        let aimed = super::Aimed {
            action: InboundAction::RunCommand { command },
            adopted: None,
        };
        match handle(aimed, &super::Target::Telegram, &mut async_cx) {
            super::Effect::Reply(reply) => reply.text,
            _ => panic!("a command is answered"),
        }
    };
    let again = run(ControlCommand::Task(TaskCommand::Start(Ordinal(1))));
    assert_eq!(
        again,
        crate::remote_channel::command::render_error(
            &crate::control::result::ControlError::TaskNotBacklog
        )
    );
    cx.update(|cx| {
        let state = &cx
            .global::<crate::runs::GlobalTasks>()
            .get(&task)
            .unwrap()
            .lifecycle;
        assert_ne!(*state, daruda_store::tasks::Lifecycle::Backlog);
    });
}

/// A start that fails after the reply — here git, scripted to fail, refuses
/// a branch that already exists — reaches the phone as a notice of its own.
/// That real git refuses it is `daruda_project`'s
/// `add_worktree_with_existing_branch_fails_cleanly`.
#[gpui::test]
async fn a_task_start_that_fails_later_is_told_as_a_notice(cx: &mut gpui::TestAppContext) {
    use crate::control::spec::{ControlCommand, Ordinal, TaskCommand};
    use crate::lane::git::{FakeFailure, FakeRepo, GitOp};
    use gpui::BorrowAppContext as _;
    let fake = crate::test_support::fake_git(cx);
    let fixture = crate::test_support::workspace_for_control(cx);
    fake.add_repo(FakeRepo::at(fixture.root()).branch("main"));
    fake.fail(
        fixture.root(),
        GitOp::AddLane,
        FakeFailure::Exit {
            code: Some(128),
            stderr: "fatal: a branch named 'main' already exists".into(),
        },
    );
    fixture
        .workspace
        .update(cx, |ws, cx| ws.reconcile_bootstrapped_lanes_for_test(cx));
    cx.run_until_parked();
    let created = fixture.workspace.update(cx, |ws, cx| {
        let project = ws.control_active_lane().project;
        ws.control_task_create(
            project,
            crate::test_support::new_task("Clash", "p", None),
            cx,
        )
        .expect("created")
    });
    cx.run_until_parked();
    let task = created.await.expect("recorded");
    let mut outbound = cx.update(|cx| {
        let spec = cx
            .global::<crate::runs::GlobalTasks>()
            .get(&task)
            .unwrap()
            .spec
            .clone();
        crate::runs::seed::commit_now(
            vec![daruda_store::tasks::TaskEvent::Edited {
                id: task.clone(),
                edit: daruda_store::tasks::machine::TaskEdit {
                    title: spec.title,
                    prompt: spec.prompt,
                    notes: spec.notes,
                    base_worktree_path: spec.base_worktree_path,
                    branch: Some("main".into()),
                    run_in: None,
                    project: None,
                    flow: None,
                },
                at: chrono::Utc::now(),
            }],
            cx,
        );
        crate::settings_store::SettingsStore::init(cx);
        cx.update_global::<crate::settings_store::SettingsStore, _>(|store, _| {
            store.set_user_for_testing(daruda_config::Config {
                telegram: daruda_config::TelegramConfig {
                    enabled: true,
                    authorized_chat_id: Some(42),
                    ..Default::default()
                },
                ..daruda_config::Config::default()
            });
        });
        crate::telegram::global::install_for_test(true, Some(42), cx)
    });
    let mut async_cx = cx.to_async();
    let mut run = |command| {
        let aimed = super::Aimed {
            action: InboundAction::RunCommand { command },
            adopted: None,
        };
        match handle(aimed, &super::Target::Telegram, &mut async_cx) {
            super::Effect::Reply(reply) => reply.text,
            _ => panic!("a command is answered"),
        }
    };
    run(ControlCommand::Task(TaskCommand::List));
    let reply = run(ControlCommand::Task(TaskCommand::Start(Ordinal(1))));
    assert_eq!(
        reply,
        crate::surface::strings::control::task_starting(
            crate::surface::strings::control::approval_quoted(
                daruda_control_types::agent_text::isolate("Clash")
            )
        )
    );
    cx.run_until_parked();

    let expected = crate::surface::strings::control::task_start_failed(
        crate::surface::strings::control::approval_quoted(
            daruda_control_types::agent_text::isolate("Clash"),
        ),
        crate::surface::strings::control::error_task_start_failed(),
    );
    let mut notices = Vec::new();
    while let Ok(message) = outbound.try_recv() {
        if let crate::remote_channel::bridge::Outbound::Notice(text) = message {
            notices.push(text);
        }
    }
    assert_eq!(notices, vec![expected]);
    assert_eq!(
        crate::test_support::added_lane(&fake).new_branch.as_deref(),
        Some("main"),
        "the start asked git for the existing branch"
    );
    assert_eq!(fake.calls().len(), 1, "the only write: {:?}", fake.calls());
}

/// An approval tap is settled under the surface it came from, so the card
/// says "in Slack" rather than "on phone" for a desktop Slack client.
#[gpui::test]
fn a_tap_is_attributed_to_its_channel_kind(cx: &mut gpui::TestAppContext) {
    use crate::control::approval::Answerer;
    cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        assert_eq!(
            super::Target::Remote("work".into()).answerer(cx),
            Some(Answerer::Slack)
        );
        assert_eq!(
            super::Target::Remote("personal".into()).answerer(cx),
            Some(Answerer::Discord)
        );
        assert_eq!(
            super::Target::Telegram.answerer(cx),
            Some(Answerer::Telegram)
        );
        assert_eq!(super::Target::Remote("missing".into()).answerer(cx), None);
    });
}

/// A tap on a flow question answered elsewhere is told so, not that the
/// flow stopped waiting for it.
#[test]
fn a_late_flow_answer_names_why_it_was_not_used() {
    use crate::runs::AnswerRefused;
    use crate::surface::strings as s;
    assert_eq!(super::flow_answer_label(Ok(())), s::flow::ask_answered());
    assert_eq!(
        super::flow_answer_label(Err(AnswerRefused::AlreadyAnswered)),
        s::flow::ask_already_answered()
    );
    for refused in [AnswerRefused::NotOpen, AnswerRefused::Closed] {
        assert_eq!(
            super::flow_answer_label(Err(refused)),
            s::flow::ask_closed()
        );
    }
}

#[gpui::test]
async fn unsupported_actions_do_not_require_connection_state(cx: &mut gpui::TestAppContext) {
    let effect = handle(
        super::Aimed {
            action: InboundAction::Unsupported,
            adopted: None,
        },
        &super::Target::Remote("missing".into()),
        &mut cx.to_async(),
    );
    assert!(matches!(effect, super::Effect::None));
}

/// Every arm that resolves against an ordinal table must survive the
/// connection being revoked mid-flight: an answer with no table to render
/// against is dropped, never panicked on.
#[gpui::test]
async fn a_revoked_connection_drops_the_answer_instead_of_panicking(cx: &mut gpui::TestAppContext) {
    let pane = PaneRef {
        workspace: Default::default(),
        pane: 1,
    };
    // The bridge installed and running, but this channel revoked: the
    // shape the removed `.expect("current remote connection")` crashed on.
    cx.update(|cx| {
        crate::settings_store::SettingsStore::init(cx);
        crate::remote_channel::global::install(cx);
    });
    for action in [
        InboundAction::NoTarget,
        InboundAction::SelectTarget { pane },
        InboundAction::UnclaimedSlash {
            name: "usage".into(),
            suggestion: None,
        },
    ] {
        let effect = handle(
            super::Aimed {
                action,
                adopted: None,
            },
            &super::Target::Remote("missing".into()),
            &mut cx.to_async(),
        );
        assert!(matches!(effect, super::Effect::None));
    }
}

/// A pick moved by `/use` reaches the app home the person views, so its
/// selected mark follows; a `/use` that leaves the pick as it was does not
/// rebuild the page.
#[gpui::test]
fn a_use_command_rebuilds_the_viewed_app_home(cx: &mut gpui::TestAppContext) {
    use crate::control::spec::{ControlCommand, UseTarget};
    use crate::remote_channel::global::RemoteChannels;
    cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        crate::remote_channel::home::driver::opened("work", "1", "3", cx);
        let picked = PaneRef {
            workspace: Default::default(),
            pane: 7,
        };
        RemoteChannels::select("work", Some(picked), cx);
    });
    cx.run_until_parked();
    let target = Target::Remote("work".into());
    for (label, scheduled) in [("the pick cleared", true), ("cleared already", false)] {
        // Checked inside the update that ran the command: the rebuild runs
        // when it ends.
        cx.update(|cx| {
            let aimed = aim_in_app(
                Routed::Ready(InboundAction::RunCommand {
                    command: ControlCommand::Use(UseTarget::Clear),
                }),
                cx,
            );
            let effect = handle_in_app(aimed, &target, cx);
            assert!(matches!(effect, Effect::Reply(_)), "{label}: answered");
            let channels = cx.global::<RemoteChannels>();
            assert_eq!(
                channels.connections["work"].core.command_state().selected(),
                None,
                "{label}"
            );
            assert_eq!(
                channels.home.rebuild_scheduled_for_test(),
                scheduled,
                "{label}"
            );
        });
        cx.run_until_parked();
    }
}
