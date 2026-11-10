use super::*;
use crate::remote_channel::command::RenderedReply;
use crate::remote_channel::{
    lock,
    runtime::{BotSession, WorkerPayload},
    transport::Credentials,
};
use daruda_config::remote::{ChannelConfig, ChannelKind, RemoteRecipient};
use daruda_store::inbox::{InboxLease, InboxSeed, InboxStore};
use gpui::TestAppContext;

fn open_bot(
    root: &std::path::Path,
    seed: impl FnOnce() -> std::io::Result<InboxSeed>,
) -> Arc<BotSession> {
    let lease = crate::test_support::once_released(
        || InboxLease::try_acquire(root, &lock::digest_of("shared-bot")),
        |lease| matches!(lease, Ok(None)),
    )
    .unwrap()
    .unwrap();
    let inbox = InboxStore::open(lease, "test", seed).unwrap();
    Arc::new(BotSession::new(
        Arc::new(Credentials {
            bot: "shared-bot".into(),
            app: Some("app".into()),
        }),
        Arc::new(inbox),
        Default::default(),
    ))
}

/// Claim through the production inbox path, as `Some(bot)` when it may act.
async fn claim_incoming(
    id: &str,
    generation: uuid::Uuid,
    event_id: &str,
    cx: &mut gpui::AsyncApp,
) -> Option<Arc<BotSession>> {
    match super::claim_incoming(
        id,
        generation,
        event_id,
        &crate::runs::RunsHost::claim_inbound,
        cx,
    )
    .await
    {
        Claimed::Ready(bot) => Some(bot),
        Claimed::Skip | Claimed::Retry => None,
    }
}

fn wait_release(root: &std::path::Path) {
    let lease = crate::test_support::once_released(
        || InboxLease::try_acquire(root, &lock::digest_of("shared-bot")),
        |lease| matches!(lease, Ok(None)),
    )
    .unwrap()
    .unwrap();
    drop(lease);
}

fn reopen_requested(id: &str, cx: &mut TestAppContext) -> bool {
    cx.update(
        |cx| match &cx.global::<RemoteChannels>().connections[id].worker {
            super::super::WorkerState::Running { worker } => worker.reopen_requested(),
            _ => panic!("the connection's worker was replaced"),
        },
    )
}

/// The defect this guards: recovery used to start a second worker while the
/// first still held the bot's lease, and the newcomer reported this process's
/// own bot as held elsewhere. The same worker now reopens its own writer.
#[gpui::test]
async fn a_failed_claim_has_its_own_worker_reopen_the_inbox_without_losing_history(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let generation = cx.update(|cx| install_connection("work", bot.clone(), cx));
    cx.update(|cx| run_worker("work", cx));
    let target = crate::remote_channel::bridge::PaneRef {
        workspace: Default::default(),
        pane: 7,
    };
    cx.update(|cx| {
        cx.global_mut::<RemoteChannels>()
            .connections
            .get_mut("work")
            .unwrap()
            .core
            .command_state_mut()
            .select(Some(target))
    });
    let mut app = cx.to_async();
    assert!(
        claim_incoming("work", generation, "committed", &mut app)
            .await
            .is_some()
    );
    let result = finish_claim(
        "work",
        generation,
        bot.clone(),
        std::future::ready(Err(daruda_store::inbox::CommitError::Io(
            std::io::Error::other("temporary storage failure"),
        ))),
        &mut app,
    )
    .await;
    assert!(matches!(result, Claimed::Retry));
    cx.update(|cx| {
        let connection = &cx.global::<RemoteChannels>().connections["work"];
        assert_eq!(
            connection.status,
            crate::remote_channel::runtime::Status::Failed
        );
        assert_eq!(connection.generation, generation);
        assert!(connection.bot.is_none());
        assert!(!connection.can_send());
    });
    assert!(
        !reopen_requested("work", cx),
        "the reopen waits out the claim backoff"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(reopen_requested("work", cx));
    cx.update(|cx| {
        assert_eq!(
            cx.global_mut::<RemoteChannels>()
                .connections
                .get_mut("work")
                .unwrap()
                .core
                .command_state_mut()
                .selected(),
            Some(target)
        )
    });
    assert!(!bot.is_live());
    assert!(
        InboxLease::try_acquire(root.path(), &lock::digest_of("shared-bot"))
            .unwrap()
            .is_none()
    );
    drop(bot);
    let reopened = open_bot(root.path(), || panic!("recovery must not reimport"));
    let generation = cx.update(|cx| {
        let connection = cx
            .global_mut::<RemoteChannels>()
            .connections
            .get_mut("work")
            .unwrap();
        connection.publish_bot(reopened.clone());
        connection.status = crate::remote_channel::runtime::Status::Connected;
        connection.generation
    });
    assert!(
        claim_incoming("work", generation, "committed", &mut app)
            .await
            .is_none()
    );
    assert!(
        claim_incoming("work", generation, "new", &mut app)
            .await
            .is_some()
    );
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(reopened);
    wait_release(root.path());
}

#[gpui::test]
fn an_old_inbox_failure_cannot_restart_a_replacement_or_finish_credential_updates(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        let first_root = tempfile::tempdir().unwrap();
        let old = open_bot(first_root.path(), || Ok(InboxSeed::default()));
        let generation = install_connection("work", old.clone(), cx);
        let config = cx.global::<RemoteChannels>().connections["work"]
            .config
            .clone();
        RemoteChannels::suspend(config.clone(), cx);
        RemoteChannels::suspend(config, cx);
        let current_generation = cx.global::<RemoteChannels>().connections["work"].generation;
        RemoteChannels::recover_inbox("work", generation, &old, cx);
        assert_eq!(
            cx.global::<RemoteChannels>().connections["work"].generation,
            current_generation
        );
        assert!(matches!(
            cx.global::<RemoteChannels>().connections["work"].worker,
            super::super::WorkerState::UpdatingCredential { pending: 2 }
        ));
        let current_root = tempfile::tempdir().unwrap();
        let current = open_bot(current_root.path(), || Ok(InboxSeed::default()));
        let current_generation = install_connection("work", current.clone(), cx);
        RemoteChannels::recover_inbox("work", generation, &old, cx);
        let connection = &cx.global::<RemoteChannels>().connections["work"];
        assert_eq!(connection.generation, current_generation);
        assert!(connection.can_send());
        assert!(current.is_live());
        cx.global_mut::<RemoteChannels>().connections.clear();
        drop(old);
        drop(current);
        wait_release(first_root.path());
        wait_release(current_root.path());
    });
}

/// Stand in a running worker with no thread behind it.
fn run_worker(id: &str, cx: &mut App) {
    cx.global_mut::<RemoteChannels>()
        .connections
        .get_mut(id)
        .unwrap()
        .worker = super::super::WorkerState::Running {
        worker: crate::remote_channel::runtime::Worker::detached(),
    };
}

fn install_connection(id: &str, bot: Arc<BotSession>, cx: &mut App) -> uuid::Uuid {
    install_connection_with_queue(id, bot, cx).0
}

/// [`install_connection`], keeping the outgoing queue so a test can send.
fn install_connection_with_queue(
    id: &str,
    bot: Arc<BotSession>,
    cx: &mut App,
) -> (
    uuid::Uuid,
    futures::channel::mpsc::UnboundedReceiver<super::super::Pending>,
) {
    SettingsStore::init(cx);
    let mut config = daruda_config::Config::default();
    let mut channel = ChannelConfig::new(id.into(), ChannelKind::Slack);
    channel.enabled = true;
    channel.recipient = Some(RemoteRecipient {
        user_id: "user".into(),
        conversation_id: "room".into(),
        scope_id: "scope".into(),
    });
    config.remote.channels.push(channel);
    cx.global_mut::<SettingsStore>()
        .set_user_for_testing(config);
    let (events, _) = futures::channel::mpsc::unbounded();
    let (outbound, queue) = futures::channel::mpsc::unbounded();
    cx.set_global(RemoteChannels {
        connections: Default::default(),
        events,
        outbound,
        applied: None,
        threads: crate::remote_channel::threads::state::Threads::load(),
    });
    super::super::reconcile(cx);
    let connection = cx
        .global_mut::<RemoteChannels>()
        .connections
        .get_mut(id)
        .unwrap();
    connection.publish_bot(bot);
    connection.status = crate::remote_channel::runtime::Status::Connected;
    (connection.generation, queue)
}

#[gpui::test]
async fn slack_redelivery_after_a_profile_uuid_handoff_has_no_second_effect_or_reply(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let _workspace = crate::test_support::workspace_for_control(cx);
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let first_id = uuid::Uuid::new_v4().to_string();
    let second_id = uuid::Uuid::new_v4().to_string();
    assert_ne!(first_id, second_id);
    std::fs::write(
        profile.path().join("remote_channels.json"),
        serde_json::to_vec(&serde_json::json!({"processed": {&first_id: ["legacy"]}})).unwrap(),
    )
    .unwrap();
    let first = open_bot(root.path(), || {
        daruda_store::inbox::migrate::remote(
            profile.path(),
            &first_id,
            channel_key(ChannelKind::Slack),
        )
    });
    let generation = cx.update(|cx| install_connection(&first_id, first.clone(), cx));
    let mut app = cx.to_async();
    let mut replies = 0;
    for _ in 0..2 {
        if let Some(bot) = claim_incoming(&first_id, generation, "event", &mut app).await {
            let incoming = transport::Incoming {
                event_id: "event".into(),
                sender: RemoteRecipient {
                    user_id: "user".into(),
                    conversation_id: "room".into(),
                    scope_id: "scope".into(),
                },
                kind: IncomingKind::Message {
                    text: "!list".into(),
                    reply_to: None,
                },
            };
            let effect =
                app.update(|cx| dispatch_claimed(&first_id, generation, &bot, &incoming, cx));
            assert!(matches!(effect, Some(Effect::Reply(_))));
            replies += 1;
        }
    }
    assert_eq!(replies, 1);
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(first);
    let second = open_bot(root.path(), || {
        panic!("a new profile must not import its config id again")
    });
    let generation = cx.update(|cx| install_connection(&second_id, second.clone(), cx));
    assert!(
        claim_incoming(&second_id, generation, "event", &mut app)
            .await
            .is_none()
    );
    assert!(
        claim_incoming(&second_id, generation, "legacy", &mut app)
            .await
            .is_none()
    );
    assert_eq!(replies, 1);
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(second);
    wait_release(root.path());
}

#[gpui::test]
async fn worker_revocation_while_a_claim_waits_prevents_effects_before_status_is_consumed(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let generation = cx.update(|cx| install_connection("profile-local-id", bot.clone(), cx));
    let queued = crate::runs::RunsHost::claim_inbound(
        &bot.inbox,
        channel_key(ChannelKind::Slack),
        daruda_store::inbox::InboxId::Opaque("pending".into()),
    );
    let (entered, blocked) = smol::channel::bounded(1);
    let (release, resume) = smol::channel::bounded(1);
    let waiting_bot = bot.clone();
    let waiting = cx.update(|cx| {
        cx.spawn(async move |cx| {
            finish_claim(
                "profile-local-id",
                generation,
                waiting_bot,
                async move {
                    let committed = queued.await;
                    entered.send(()).await.unwrap();
                    resume.recv().await.unwrap();
                    committed
                },
                cx,
            )
            .await
        })
    });
    blocked.recv().await.unwrap();
    bot.revoke();
    cx.update(|cx| {
        let connection = &cx.global::<RemoteChannels>().connections["profile-local-id"];
        assert_eq!(
            connection.status,
            crate::remote_channel::runtime::Status::Connected
        );
        assert!(!connection.can_send());
    });
    assert!(
        InboxLease::try_acquire(root.path(), &lock::digest_of("shared-bot"))
            .unwrap()
            .is_none()
    );
    release.send(()).await.unwrap();
    assert!(matches!(waiting.await, Claimed::Skip));
    assert!(matches!(
        smol::block_on(bot.inbox.claim(
            channel_key(ChannelKind::Slack),
            daruda_store::inbox::InboxId::Opaque("pending".into())
        )),
        Err(daruda_store::inbox::CommitError::Rejected(
            daruda_store::inbox::InboxRejected::Duplicate { .. }
        ))
    ));
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(bot);
    wait_release(root.path());
}

fn callback() -> IncomingKind {
    IncomingKind::Callback {
        data: "token".into(),
        message_id: "12.34".into(),
        original: "Allow write?".into(),
    }
}

/// A tap whose token names nothing live, on a message no settlement will
/// edit, loses its buttons in place; otherwise they stay tappable forever.
#[test]
fn a_dead_tap_on_an_untracked_card_loses_its_buttons() {
    assert_eq!(
        feedback_for(Effect::None, callback(), Tap::Untracked),
        Feedback::Edit {
            message_id: "12.34".into(),
            original: "Allow write?".into(),
            label: s::remote_channel::stale_callback(),
        }
    );
    assert_eq!(
        feedback_for(
            Effect::Feedback {
                label: s::control::approval_already_answered(),
                edit: Edit::StaleCard,
            },
            callback(),
            Tap::Untracked,
        ),
        Feedback::Edit {
            message_id: "12.34".into(),
            original: "Allow write?".into(),
            label: s::control::approval_already_answered(),
        }
    );
}

/// A tap racing a settlement edit is answered without rewriting the card, so
/// the settled result is not replaced by a competing label.
#[test]
fn a_dead_tap_on_a_settled_card_does_not_overwrite_the_result() {
    assert_eq!(
        feedback_for(Effect::None, callback(), Tap::Settled),
        Feedback::Send(Message::plain(s::remote_channel::stale_callback(), None))
    );
}

#[test]
fn a_managed_card_leaves_editing_to_the_ledger_and_a_selection_keeps_its_buttons() {
    assert!(matches!(
        feedback_for(
            Effect::Feedback {
                label: "Allowed".into(),
                edit: Edit::ManagedCard,
            },
            callback(),
            Tap::Untracked,
        ),
        Feedback::Nothing
    ));
    // A listing row stays tappable — picking another row is ordinary use.
    assert_eq!(
        feedback_for(
            Effect::Feedback {
                label: "Now talking to lane 2".into(),
                edit: Edit::KeepButtons,
            },
            callback(),
            Tap::Untracked,
        ),
        Feedback::Send(Message::plain("Now talking to lane 2".into(), None)),
    );
}

/// A plain message has no keyboard to strip, so nothing is owed unless the
/// effect produced an answer of its own.
#[test]
fn a_plain_message_is_never_edited() {
    let text = IncomingKind::Message {
        text: "ship it".into(),
        reply_to: None,
    };
    assert_eq!(
        feedback_for(Effect::None, text, Tap::Absent),
        Feedback::Nothing
    );
    assert_eq!(
        feedback_for(
            Effect::Reply(RenderedReply {
                text: "done".into(),
                keyboard: None,
            }),
            IncomingKind::Message {
                text: "ship it".into(),
                reply_to: None,
            },
            Tap::Absent,
        ),
        Feedback::Send(Message::plain("done".into(), None)),
    );
}

fn from(user: &str, event_id: &str, generation: uuid::Uuid) -> WorkerEvent {
    WorkerEvent {
        id: "work".into(),
        generation,
        payload: WorkerPayload::Incoming(transport::Incoming {
            event_id: event_id.into(),
            sender: RemoteRecipient {
                user_id: user.into(),
                conversation_id: "room".into(),
                scope_id: "scope".into(),
            },
            kind: IncomingKind::Message {
                text: "hello".into(),
                reply_to: None,
            },
        }),
    }
}

/// Slack and Discord acknowledge an event before it reaches the claim, so a
/// command dropped on a failed claim is never sent again. It waits for the
/// reopened inbox instead, with the commands after it queued behind it.
/// The sender is a stranger so the claimed commands stop short of a reply.
#[gpui::test]
async fn a_command_whose_claim_failed_is_claimed_again_on_the_reopened_inbox(
    cx: &mut TestAppContext,
) {
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

    events
        .unbounded_send(from("stranger", "first", generation))
        .unwrap();
    events
        .unbounded_send(from("stranger", "second", generation))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *attempts.borrow(),
        ["first"],
        "a later command must not be claimed ahead of a held one"
    );
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.run_until_parked();
    assert!(reopen_requested("work", cx));

    drop(first);
    let reopened = open_bot(root.path(), || panic!("recovery must not reimport"));
    events
        .unbounded_send(WorkerEvent {
            id: "work".into(),
            generation,
            payload: WorkerPayload::Ready(reopened.clone()),
        })
        .unwrap();
    // A real claim completes on the inbox's writer thread, outside the
    // executor, so parking alone does not mean the backlog has drained.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while attempts.borrow().len() < 3 && std::time::Instant::now() < deadline {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(*attempts.borrow(), ["first", "first", "second"]);
    for event in ["first", "second"] {
        assert!(matches!(
            smol::block_on(reopened.inbox.claim(
                channel_key(ChannelKind::Slack),
                InboxId::Opaque(event.into())
            )),
            Err(CommitError::Rejected(InboxRejected::Duplicate { .. }))
        ));
    }
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(events);
    cx.run_until_parked();
    drop(reopened);
    wait_release(root.path());
}

#[gpui::test]
fn callbacks_before_send_completion_cannot_schedule_a_stale_edit(cx: &mut TestAppContext) {
    use crate::control::approval::{self, ApprovalOutcome};
    use crate::remote_channel::cards::tests::EditCapture;
    cx.update(|cx| {
        let mut outgoing = crate::remote_channel::global::tests::setup(cx);
        cx.set_global(EditCapture::default());
        let (_id, answer) =
            approval::request("In flight".into(), super::super::Delivery::Explicit, cx);
        let pending = outgoing.try_recv().unwrap();
        let _prepared = super::super::outbound::prepare(&pending, cx).unwrap();
        let crate::remote_channel::bridge::Outbound::Approval(prompt) = &pending.outbound else {
            panic!("approval");
        };
        for (index, (_, token)) in prompt.buttons.iter().enumerate() {
            let incoming = transport::Incoming {
                event_id: format!("tap-{index}"),
                sender: pending.recipient.clone(),
                kind: IncomingKind::Callback {
                    data: token.clone(),
                    message_id: "early-message".into(),
                    original: prompt.summary.clone(),
                },
            };
            let effect =
                dispatch_claimed(&pending.id, pending.generation, &pending.bot, &incoming, cx)
                    .unwrap();
            let connection = &cx.global::<RemoteChannels>().connections[&pending.id];
            assert!(!connection.desk.borrow().untracked(&"early-message".into()));
            assert!(!matches!(
                feedback_for(effect, incoming.kind, Tap::Settled),
                Feedback::Edit { .. }
            ));
        }
        assert_eq!(answer.try_recv(), Ok(ApprovalOutcome::Approved));
        super::super::outbound::finish(
            &pending,
            None,
            crate::remote_channel::cards::SendOutcome::Delivered(vec![
                crate::remote_channel::transport::SentPart {
                    id: "early-message".into(),
                    text: prompt.summary.clone(),
                    buttons: true,
                },
            ]),
            cx,
        );
        assert_eq!(cx.global::<EditCapture>().edits.len(), 1);
    });
}

/// An outbound preference does not revoke a command already entering the inbox.
#[gpui::test]
async fn a_presence_edit_during_a_durable_claim_preserves_the_command(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let generation = cx.update(|cx| install_connection("work", bot.clone(), cx));
    let claim = crate::runs::RunsHost::claim_inbound(
        &bot.inbox,
        channel_key(ChannelKind::Slack),
        InboxId::Opaque("presence-edit".into()),
    );
    let (entered, blocked) = smol::channel::bounded(1);
    let (release, resume) = smol::channel::bounded(1);
    let claimed_bot = bot.clone();
    let pending = cx.update(|cx| {
        cx.spawn(async move |cx| {
            finish_claim(
                "work",
                generation,
                claimed_bot,
                async move {
                    let committed = claim.await;
                    entered.send(()).await.unwrap();
                    resume.recv().await.unwrap();
                    committed
                },
                cx,
            )
            .await
        })
    });
    blocked.recv().await.unwrap();
    cx.update(|cx| {
        let mut config = SettingsStore::global(cx).user().clone();
        config.remote.channels[0].only_when_away = false;
        cx.global_mut::<SettingsStore>()
            .set_user_for_testing(config);
        super::super::reconcile(cx);
    });
    release.send(()).await.unwrap();
    assert!(
        matches!(pending.await, Claimed::Ready(_)),
        "presence must not consume an inbound command"
    );
    cx.update(|cx| cx.global_mut::<RemoteChannels>().connections.clear());
    drop(bot);
    wait_release(root.path());
}

/// An approval whose card every channel has started sending.
struct SendingApproval {
    id: crate::control::approval::ApprovalId,
    answer: smol::channel::Receiver<crate::control::approval::ApprovalOutcome>,
    outgoing: futures::channel::mpsc::UnboundedReceiver<super::super::outbound::Pending>,
    /// Each channel's pending card with its approve token.
    sending: Vec<(super::super::outbound::Pending, String)>,
}

fn sending_approval(cx: &mut App) -> SendingApproval {
    let mut outgoing = crate::remote_channel::global::tests::setup(cx);
    cx.set_global(crate::remote_channel::cards::tests::EditCapture::default());
    let (id, answer) = crate::control::approval::request(
        "Create worktree".into(),
        super::super::Delivery::Explicit,
        cx,
    );
    let mut sending = Vec::new();
    while let Ok(pending) = outgoing.try_recv() {
        super::super::outbound::prepare(&pending, cx).unwrap();
        let crate::remote_channel::bridge::Outbound::Approval(prompt) = &pending.outbound else {
            panic!("approval");
        };
        let approve = prompt.buttons[0].1.clone();
        sending.push((pending, approve));
    }
    assert_eq!(sending.len(), 2);
    SendingApproval {
        id,
        answer,
        outgoing,
        sending,
    }
}

/// Tap `token` on `message_id`, returning what the sender is owed.
fn tap_card(
    pending: &super::super::outbound::Pending,
    token: &str,
    message_id: &str,
    cx: &mut App,
) -> Feedback {
    let incoming = transport::Incoming {
        event_id: format!("tap-{message_id}"),
        sender: pending.recipient.clone(),
        kind: IncomingKind::Callback {
            data: token.into(),
            message_id: message_id.into(),
            original: "Create worktree".into(),
        },
    };
    let effect =
        dispatch_claimed(&pending.id, pending.generation, &pending.bot, &incoming, cx).unwrap();
    let connection = &cx.global::<RemoteChannels>().connections[&pending.id];
    let tap = if connection.desk.borrow().untracked(&message_id.into()) {
        Tap::Untracked
    } else {
        Tap::Settled
    };
    feedback_for(effect, incoming.kind, tap)
}

/// A send that ends without a receipt may still have reached the person: the
/// card it posted keeps answering, and the pump does not offer a second one.
#[gpui::test]
fn a_remote_card_sent_without_a_receipt_still_answers_its_tap(cx: &mut TestAppContext) {
    use crate::control::approval::{self, ApprovalOutcome};
    use crate::remote_channel::cards::tests::EditCapture;
    cx.update(|cx| {
        let SendingApproval {
            answer,
            mut outgoing,
            sending,
            ..
        } = sending_approval(cx);
        for (pending, _) in &sending {
            super::super::outbound::finish(
                pending,
                None,
                crate::remote_channel::cards::SendOutcome::Unconfirmed,
                cx,
            );
        }

        approval::relay_outstanding(cx);
        assert!(
            outgoing.try_recv().is_err(),
            "the person may see these cards"
        );

        let (pending, approve) = &sending[0];
        assert_eq!(
            tap_card(pending, approve, "unconfirmed", cx),
            Feedback::Nothing
        );
        assert_eq!(answer.try_recv(), Ok(ApprovalOutcome::Approved));
        assert!(cx.global::<EditCapture>().edits.is_empty());
    });
}

/// Settling a card that has no receipt forgets its tokens: nothing will edit
/// the message it may have posted, so a later tap takes its buttons off.
#[gpui::test]
fn settling_a_remote_card_without_a_receipt_leaves_its_buttons_to_a_stale_tap(
    cx: &mut TestAppContext,
) {
    use crate::control::approval::{self, ApprovalOutcome};
    use crate::remote_channel::cards::tests::EditCapture;
    cx.update(|cx| {
        let SendingApproval {
            id,
            answer,
            outgoing: _outgoing,
            sending,
        } = sending_approval(cx);
        for (pending, _) in &sending {
            super::super::outbound::finish(
                pending,
                None,
                crate::remote_channel::cards::SendOutcome::Unconfirmed,
                cx,
            );
        }
        for connection in cx.global::<RemoteChannels>().connections.values() {
            assert_eq!(connection.desk.borrow().approval_token_count(), 2);
        }

        assert!(approval::resolve_desktop(id.0, "approve", cx));
        assert_eq!(answer.try_recv(), Ok(ApprovalOutcome::Approved));
        for connection in cx.global::<RemoteChannels>().connections.values() {
            assert_eq!(connection.desk.borrow().approval_token_count(), 0);
            assert!(
                !connection
                    .desk
                    .borrow()
                    .contains(&crate::remote_channel::cards::CardId::Approval(id))
            );
        }

        let (pending, approve) = &sending[0];
        assert_eq!(
            tap_card(pending, approve, "unconfirmed", cx),
            Feedback::Edit {
                message_id: "unconfirmed".into(),
                original: "Create worktree".into(),
                label: s::control::approval_already_answered(),
            }
        );
        assert!(cx.global::<EditCapture>().edits.is_empty());
    });
}

/// A send that never left posted nothing: its claim and tokens go together,
/// and the pump offers the card once more on each channel.
#[gpui::test]
fn a_remote_card_that_never_left_is_offered_once_more(cx: &mut TestAppContext) {
    use crate::control::approval;
    cx.update(|cx| {
        let SendingApproval {
            id,
            answer: _answer,
            mut outgoing,
            sending,
        } = sending_approval(cx);
        for (pending, _) in &sending {
            super::super::outbound::finish(
                pending,
                None,
                crate::remote_channel::cards::SendOutcome::NotSent,
                cx,
            );
        }
        for connection in cx.global::<RemoteChannels>().connections.values() {
            assert_eq!(connection.desk.borrow().approval_token_count(), 0);
            assert!(
                !connection
                    .desk
                    .borrow()
                    .contains(&crate::remote_channel::cards::CardId::Approval(id))
            );
        }

        approval::relay_outstanding(cx);
        let mut offered = Vec::new();
        while let Ok(pending) = outgoing.try_recv() {
            offered.push(pending.id);
        }
        offered.sort();
        assert_eq!(offered, ["personal", "work"]);
        approval::relay_outstanding(cx);
        assert!(outgoing.try_recv().is_err(), "exactly once");
        assert!(approval::withdraw(id, cx));
    });
}

/// A reply in a pane's thread reaches that pane, whatever the plain-text
/// target is; a thread no open pane holds is refused instead of guessed,
/// unless the reply is one of daruda's own commands.
#[gpui::test]
fn a_thread_reply_reaches_its_pane_and_an_unplaceable_thread_is_refused(cx: &mut TestAppContext) {
    use crate::remote_channel::bridge::PaneRef;
    use crate::remote_channel::threads::route::ThreadReply;
    use crate::remote_channel::threads::{ConversationKey, Scope};

    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let set_session = |session: &str, cx: &mut TestAppContext| {
        fixture.workspace.update(cx, |ws, cx| {
            ws.set_pane_session_for_test(fixture.pane(), session, cx);
            let pane = PaneRef {
                workspace: ws.uuid(),
                pane: fixture.pane(),
            };
            let key: ConversationKey = ws.remote_conversation(fixture.pane(), cx).unwrap().key;
            (pane, key)
        })
    };
    let (pane, key) = set_session("s1", cx);
    let generation = cx.update(|cx| {
        let generation = install_connection("work", bot.clone(), cx);
        let scope = Scope {
            connection: "work".into(),
            bot: lock::digest_of("shared-bot"),
            conversation: "room".into(),
        };
        let channels = cx.global_mut::<RemoteChannels>();
        channels
            .threads
            .remember(&scope, "t1".into(), key, "100.1".into(), 1);
        channels
            .connections
            .get_mut("work")
            .unwrap()
            .core
            .record_sent("50.5".into(), pane);
        generation
    });

    cx.update(|cx| {
        let cases = [
            (
                Some("100.1"),
                ThreadReply::Pane(pane),
                "the pane's own thread",
            ),
            (
                Some("50.5"),
                ThreadReply::Untracked,
                "a ping sent before threads",
            ),
            (None, ThreadReply::Untracked, "the top of the chat"),
            (
                Some("999.9"),
                ThreadReply::Unknown,
                "a thread no pane started",
            ),
        ];
        for (thread, want, label) in cases {
            assert_eq!(
                route::route_reply("work", &bot, thread, cx),
                want,
                "{label}"
            );
        }
    });

    set_session("s2", cx);
    cx.update(|cx| {
        assert_eq!(
            route::route_reply("work", &bot, Some("100.1"), cx),
            ThreadReply::Closed,
            "the thread's conversation is no longer open"
        );
        let reply = |text: &str, thread: &str, cx: &mut App| {
            let incoming = transport::Incoming {
                event_id: format!("{thread}-{text}"),
                sender: RemoteRecipient {
                    user_id: "user".into(),
                    conversation_id: "room".into(),
                    scope_id: "scope".into(),
                },
                kind: IncomingKind::Message {
                    text: text.into(),
                    reply_to: Some(thread.into()),
                },
            };
            dispatch_claimed("work", generation, &bot, &incoming, cx).unwrap()
        };
        for (thread, refusal) in [
            ("100.1", s::remote_channel::thread_closed()),
            ("999.9", s::remote_channel::thread_unknown()),
        ] {
            assert!(
                matches!(reply("ship it", thread, cx), Effect::Reply(RenderedReply { text, .. }) if text == refusal),
                "plain text in {thread} is refused"
            );
            assert!(
                !matches!(reply("!list", thread, cx), Effect::Reply(RenderedReply { text, .. }) if text == refusal),
                "a command in {thread} still runs"
            );
        }
    });
}

/// A tap on a thread root's stop button stops the pane its thread speaks for,
/// found by thread so it survives a restart; a thread whose conversation is
/// not open says so. Its answer goes back into that thread.
#[gpui::test]
fn a_root_stop_tap_reaches_the_threads_pane_and_answers_in_the_thread(cx: &mut TestAppContext) {
    use crate::remote_channel::threads::{ConversationKey, Scope, stop};

    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let key: ConversationKey = fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_session_for_test(fixture.pane(), "s1", cx);
        ws.remote_conversation(fixture.pane(), cx).unwrap().key
    });
    let generation = cx.update(|cx| {
        let generation = install_connection("work", bot.clone(), cx);
        let scope = Scope {
            connection: "work".into(),
            bot: lock::digest_of("shared-bot"),
            conversation: "room".into(),
        };
        cx.global_mut::<RemoteChannels>().threads.remember(
            &scope,
            "t1".into(),
            key,
            "100.1".into(),
            1,
        );
        generation
    });
    cx.update(|cx| {
        let tap = |data: String, cx: &mut App| {
            let incoming = transport::Incoming {
                event_id: data.clone(),
                sender: RemoteRecipient {
                    user_id: "user".into(),
                    conversation_id: "room".into(),
                    scope_id: "scope".into(),
                },
                kind: IncomingKind::Callback {
                    data,
                    message_id: "100.1".into(),
                    original: String::new(),
                },
            };
            let thread = route::reply_thread(ChannelKind::Slack, &incoming.kind);
            (
                dispatch_claimed("work", generation, &bot, &incoming, cx),
                thread,
            )
        };
        let closed = s::remote_channel::thread_closed();
        fixture.workspace.update(cx, |ws, cx| {
            ws.set_pane_working_for_test(fixture.pane(), cx)
        });
        let (effect, thread) = tap(stop::token("t1"), cx);
        let effect = effect.unwrap();
        assert!(
            !matches!(&effect, Effect::Reply(RenderedReply { text, .. }) if *text == closed),
            "the open pane is found"
        );
        assert_eq!(
            fixture
                .workspace
                .update(cx, |ws, cx| ws.control_stop(fixture.pane(), cx)),
            Ok(crate::control::result::StopDisposition::AlreadyIdle),
            "the tap stopped the working pane"
        );
        // The tapped message is the thread's status root: the answer is never
        // an edit of it, whatever the stop came to.
        assert!(!matches!(
            feedback_for(effect, callback_on_root(), Tap::Untracked),
            Feedback::Edit { .. }
        ));
        assert_eq!(thread.as_deref(), Some("100.1"), "answered in the thread");
        let (effect, _) = tap(stop::token("unknown"), cx);
        assert!(
            matches!(&effect, Some(Effect::Reply(RenderedReply { text, .. })) if *text == closed),
            "a thread no open pane holds"
        );
    });
}

fn callback_on_root() -> IncomingKind {
    IncomingKind::Callback {
        data: crate::remote_channel::threads::stop::token("t1"),
        message_id: "100.1".into(),
        original: String::new(),
    }
}

/// A pane with a thread on connection "work": the fixture, the bot, the
/// thread root "100.1" holding the pane's conversation, and the queue.
struct ThreadedPane {
    fixture: crate::test_support::ControlFixture,
    bot: Arc<BotSession>,
    generation: uuid::Uuid,
    queue: futures::channel::mpsc::UnboundedReceiver<super::super::Pending>,
    _root: tempfile::TempDir,
}

fn threaded_pane(cx: &mut TestAppContext) -> ThreadedPane {
    use crate::remote_channel::threads::Scope;
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let root = tempfile::tempdir().unwrap();
    let bot = open_bot(root.path(), || Ok(InboxSeed::default()));
    let key = fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_session_for_test(fixture.pane(), "s1", cx);
        ws.remote_conversation(fixture.pane(), cx).unwrap().key
    });
    let (generation, queue) = cx.update(|cx| {
        let installed = install_connection_with_queue("work", bot.clone(), cx);
        let scope = Scope {
            connection: "work".into(),
            bot: lock::digest_of("shared-bot"),
            conversation: "room".into(),
        };
        cx.global_mut::<RemoteChannels>().threads.remember(
            &scope,
            "t1".into(),
            key,
            "100.1".into(),
            1,
        );
        installed
    });
    ThreadedPane {
        fixture,
        bot,
        generation,
        queue,
        _root: root,
    }
}

fn thread_reply(text: &str) -> transport::Incoming {
    transport::Incoming {
        event_id: format!("reply-{text}"),
        sender: RemoteRecipient {
            user_id: "user".into(),
            conversation_id: "room".into(),
            scope_id: "scope".into(),
        },
        kind: IncomingKind::Message {
            text: text.into(),
            reply_to: Some("100.1".into()),
        },
    }
}

/// The defect this guards: a bare `/stop` in a pane's thread stopped the
/// selected pane, or answered that none was selected, instead of the pane
/// the thread speaks for.
#[gpui::test]
fn a_bare_stop_in_a_panes_thread_stops_that_pane(cx: &mut TestAppContext) {
    let threaded = threaded_pane(cx);
    let pane = threaded.fixture.pane();
    threaded
        .fixture
        .workspace
        .update(cx, |ws, cx| ws.set_pane_working_for_test(pane, cx));
    cx.update(|cx| {
        dispatch_claimed(
            "work",
            threaded.generation,
            &threaded.bot,
            &thread_reply("!stop"),
            cx,
        )
        .unwrap()
    });
    assert_eq!(
        threaded
            .fixture
            .workspace
            .update(cx, |ws, cx| ws.control_stop(pane, cx)),
        Ok(crate::control::result::StopDisposition::AlreadyIdle)
    );
}

/// A reply in a pane's thread answers the question card that pane waits on;
/// the thread names the pane, not the card.
#[gpui::test]
fn a_thread_reply_answers_the_panes_live_question(cx: &mut TestAppContext) {
    use crate::remote_channel::bridge::{
        BridgePing, MessageTail, PingPrompt, QuestionButton, QuestionPrompt, QuestionPromptRef,
    };
    let mut threaded = threaded_pane(cx);
    let pane = threaded.fixture.pane_ref(cx);
    cx.update(|cx| {
        let question = QuestionPromptRef {
            session: 1,
            question_id: 4,
            field: 0,
        };
        assert!(RemoteChannels::send_ping(
            BridgePing {
                pane,
                header: "question".into(),
                tail: MessageTail::Plain("Which colour?".into()),
                prompt: Some(PingPrompt::Question(QuestionPrompt {
                    question,
                    step: 1,
                    rows: vec![vec![("Blue".into(), QuestionButton::Option(0))]],
                })),
                weight: crate::remote_channel::bridge::PingWeight::Progress,
            },
            super::super::Delivery::Explicit,
            cx,
        ));
        let pending = threaded.queue.try_recv().unwrap();
        let prepared = super::super::outbound::prepare(&pending, cx).unwrap();
        super::super::outbound::finish(
            &pending,
            Some(pane),
            crate::remote_channel::cards::SendOutcome::Delivered(
                prepared.parts(vec!["200.1".into()]),
            ),
            cx,
        );
        let effect = dispatch_claimed(
            "work",
            threaded.generation,
            &threaded.bot,
            &thread_reply("blue"),
            cx,
        )
        .unwrap();
        // The view holds no such question, so the answer is refused — in a
        // question's words, which only a routed answer produces.
        let question_answers = [
            s::notification::remote_question_stale(),
            s::notification::remote_permission_gone(),
            s::notification::remote_question_text_not_taken(),
        ];
        assert!(
            matches!(&effect, Effect::Reply(RenderedReply { text, .. }) if question_answers.contains(text)),
            "routed as an answer, not a prompt"
        );
    });
}
