use super::*;
use crate::remote_channel::bridge::{
    BridgePing, FlowAskPrompt, MessageTail, PermissionDecision, PermissionPromptRef, PingPrompt,
    QuestionButton, QuestionPrompt, QuestionPromptRef,
};
use crate::telegram::global::TelegramBridge;
use std::time::{Duration, Instant};

fn ping(header: &str, prompt: PingPrompt) -> BridgePing {
    BridgePing {
        pane: PaneRef {
            workspace: Default::default(),
            pane: 1,
        },
        header: header.into(),
        tail: MessageTail::Plain("waiting".into()),
        prompt: Some(prompt),
        weight: crate::remote_channel::bridge::PingWeight::Progress,
    }
}

/// One live card of every kind a phone can hold, on every channel.
fn offer_every_kind(cx: &mut App) -> smol::channel::Receiver<ApprovalOutcome> {
    let (_, answer) = approval::request("Approve this".into(), Delivery::Explicit, cx);
    assert!(crate::remote_channel::send_flow_ask(
        FlowAskPrompt {
            worktree: "/tree".into(),
            run_dir: "/tree/run".into(),
            ask_id: 1,
            summary: "Flow asks".into(),
            buttons: vec![("Allow".into(), PermissionDecision::Allow("once".into()))],
        },
        Delivery::Explicit,
        cx,
    ));
    let pings = [
        ping(
            "Permission",
            PingPrompt::Permission(PermissionPromptRef {
                session: 1,
                perm_id: 2,
                buttons: vec![("Allow".into(), PermissionDecision::Allow("once".into()))],
            }),
        ),
        ping(
            "Question",
            PingPrompt::Question(QuestionPrompt {
                question: QuestionPromptRef {
                    session: 1,
                    question_id: 3,
                    field: 0,
                },
                step: 1,
                rows: vec![vec![("A".into(), QuestionButton::Option(0))]],
            }),
        ),
    ];
    for ping in pings {
        cx.global::<TelegramBridge>().send(ping.clone(), cx);
        assert!(RemoteChannels::send_ping(ping, Delivery::Explicit, cx));
    }
    answer
}

fn deliver(
    remote: &mut futures::channel::mpsc::UnboundedReceiver<Pending>,
    telegram: &mut crate::telegram::global::TestOutbox,
    first_id: i64,
    cx: &mut App,
) {
    complete(begin(remote, cx), cx);
    let mut id = first_id;
    while let Some(finish) = telegram.begin_send(id, cx) {
        finish(cx);
        id += 1;
    }
}

fn labelled<'a>(channel: &str, label: &str, cx: &'a App) -> Vec<&'a str> {
    let mut originals: Vec<_> = cx
        .global::<EditCapture>()
        .edits
        .iter()
        .filter(|edit| edit.channel == channel && edit.label == label)
        .map(|edit| edit.original.as_str())
        .collect();
    originals.sort();
    originals
}

#[gpui::test]
fn quitting_cancels_every_live_card_and_finishes_queued_settlements(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let (mut remote, mut telegram) = queued(cx);
        let (settled, _answer) = approval::request("Settled".into(), Delivery::Explicit, cx);
        deliver(&mut remote, &mut telegram, 10, cx);
        cx.global_mut::<EditCapture>().fail = Some(transient());
        assert!(approval::resolve_desktop(settled.0, "approve", cx));
        cx.global_mut::<EditCapture>().fail = None;
        let _answer = offer_every_kind(cx);
        deliver(&mut remote, &mut telegram, 20, cx);
        cx.global_mut::<EditCapture>().edits.clear();

        let left = crate::remote_channel::cards::quit::begin(cx)
            .wait(Instant::now() + Duration::from_secs(10));

        assert_eq!(left, 0);
        let cancelled = crate::surface::strings::notification::card_cancelled_at_quit();
        let approved = crate::surface::strings::control::approval_approved_desktop();
        for channel in ["telegram", "work", "personal"] {
            assert_eq!(
                labelled(channel, &cancelled, cx),
                [
                    "Approve this",
                    "Flow asks",
                    "Permission\nwaiting",
                    "Question\nwaiting"
                ],
                "{channel}"
            );
            assert_eq!(labelled(channel, &approved, cx), ["Settled"], "{channel}");
        }
        assert_eq!(cx.global::<EditCapture>().edits.len(), 15);
    });
}

#[gpui::test]
fn a_card_closed_at_quit_is_not_edited_or_offered_again(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let (mut remote, mut telegram) = queued(cx);
        let (id, _answer) = approval::request("Approve this".into(), Delivery::Explicit, cx);
        deliver(&mut remote, &mut telegram, 10, cx);
        crate::remote_channel::cards::quit::begin(cx)
            .wait(Instant::now() + Duration::from_secs(10));
        assert_eq!(cx.global::<EditCapture>().edits.len(), 3);

        assert!(crate::remote_channel::cards::quit::begin(cx).is_empty());
        approval::relay_outstanding(cx);
        assert!(remote.next().now_or_never().is_none());
        assert!(telegram.try_recv().is_err());
        assert!(approval::resolve_desktop(id.0, "approve", cx));
        flush_edits(cx);
        assert_eq!(cx.global::<EditCapture>().edits.len(), 3);
    });
}

#[gpui::test]
fn a_bot_that_is_not_serving_keeps_its_cards(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let (mut remote, mut telegram) = queued(cx);
        let (id, _answer) = approval::request("Approve this".into(), Delivery::Explicit, cx);
        deliver(&mut remote, &mut telegram, 10, cx);
        cx.global_mut::<TelegramBridge>().release_claim();
        for connection in cx.global_mut::<RemoteChannels>().connections.values_mut() {
            connection.revoke_bot();
        }

        assert!(crate::remote_channel::cards::quit::begin(cx).is_empty());
        assert!(cx.global::<EditCapture>().edits.is_empty());

        for connection in cx.global_mut::<RemoteChannels>().connections.values_mut() {
            connection.publish_bot(crate::remote_channel::global::tests::ready_bot());
        }
        assert!(approval::resolve_desktop(id.0, "approve", cx));
        let mut edited: Vec<_> = cx
            .global::<EditCapture>()
            .edits
            .iter()
            .map(|edit| edit.channel.as_str())
            .collect();
        edited.sort();
        assert_eq!(
            edited,
            ["personal", "work"],
            "the same identity still owns the cards quit left open"
        );
    });
}
