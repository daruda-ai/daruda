use super::*;
use crate::remote_channel::bridge::{PermissionPromptRef, QuestionPrompt};
use daruda_store::project::WorkspaceUuid;

fn pane(n: u64) -> PaneRef {
    PaneRef {
        workspace: WorkspaceUuid::default(),
        pane: n,
    }
}

fn tokens_of(message: PreparedPing) -> Vec<String> {
    message
        .keyboard
        .map(|keyboard| {
            keyboard
                .rows
                .into_iter()
                .flatten()
                .map(|(_, token)| token)
                .collect()
        })
        .unwrap_or_default()
}

fn permission(tokens: &mut CardTokens<String>, request: u64) -> Vec<String> {
    tokens_of(tokens.mint_ping(BridgePing {
        pane: pane(1),
        header: "permission".into(),
        tail: MessageTail::Plain("write".into()),
        prompt: Some(PingPrompt::Permission(PermissionPromptRef {
            session: 1,
            perm_id: request,
            buttons: vec![("Allow".into(), PermissionDecision::Allow("once".into()))],
        })),
        weight: crate::remote_channel::bridge::PingWeight::Progress,
    }))
}

fn flow_ask(tokens: &mut CardTokens<String>, ask_id: u64) -> Vec<String> {
    tokens_of(tokens.mint_flow_ask(FlowAskPrompt {
        worktree: "/tree".into(),
        run_dir: "/tree/run".into(),
        ask_id,
        summary: "Question".into(),
        buttons: vec![("Allow".into(), PermissionDecision::Allow("once".into()))],
    }))
}

fn flow_card(ask_id: u64) -> CardId {
    CardId::FlowAsk {
        run_dir: "/tree/run".into(),
        ask_id,
    }
}

#[test]
fn identities_do_not_share_tokens() {
    let mut first = CardTokens::<String>::default();
    let mut second = CardTokens::<String>::default();
    let token = permission(&mut first, 7).remove(0);
    assert_eq!(second.route_tap(&token), Some(InboundAction::Ignore));
    assert!(matches!(
        first.route_tap(&token),
        Some(InboundAction::RespondPermission { perm_id: 7, .. })
    ));
    assert_eq!(first.route_tap(&token), Some(InboundAction::Ignore));
}

#[test]
fn settlement_keeps_late_flow_routing_separate_from_live_tokens_and_bounded() {
    let mut tokens = CardTokens::<String>::default();
    let mut oldest = None;
    let mut latest = String::new();
    for ask_id in 0..=SETTLED_CALLBACKS_CAP as u64 {
        latest = flow_ask(&mut tokens, ask_id).remove(0);
        oldest.get_or_insert(latest.clone());
        tokens.tombstone(&flow_card(ask_id));
        assert_eq!(tokens.live_count(), 0);
    }
    assert!(!tokens.is_settled(&oldest.unwrap()));
    assert!(tokens.is_settled(&latest));
    assert!(matches!(
        tokens.route_tap(&latest),
        Some(InboundAction::ResolveFlowAsk { ask_id, .. }) if ask_id == SETTLED_CALLBACKS_CAP as u64
    ));
    tokens.clear_settled();
    assert_eq!(tokens.route_tap(&latest), Some(InboundAction::Ignore));
}

/// Each bounded kind evicts only its own oldest token, so a burst of one kind
/// cannot take a live card of another off the phone.
#[test]
fn each_kind_evicts_only_its_own_oldest_token() {
    let mut tokens = CardTokens::<String>::default();
    let (approve, _) = tokens.mint_approval(ApprovalId(1));
    let first = permission(&mut tokens, 0).remove(0);
    for request in 1..=TOKENS_PER_KIND_CAP as u64 {
        permission(&mut tokens, request);
    }
    assert_eq!(tokens.route_tap(&first), Some(InboundAction::Ignore));
    assert!(matches!(
        tokens.route_tap(&approve),
        Some(InboundAction::ResolveApproval { .. })
    ));
}

/// A consumed token keeps its slot until evicted, so answering one card
/// never extends how long the oldest live card stays answerable.
#[test]
fn a_consumed_token_keeps_its_slot() {
    let mut tokens = CardTokens::<String>::default();
    let spent = permission(&mut tokens, 0).remove(0);
    let oldest_live = permission(&mut tokens, 1).remove(0);
    for request in 2..TOKENS_PER_KIND_CAP as u64 {
        permission(&mut tokens, request);
    }
    assert!(tokens.route_tap(&spent).is_some());
    permission(&mut tokens, 100);
    assert!(
        tokens.live.contains_key(&oldest_live),
        "the spent token's slot was the one evicted"
    );
    permission(&mut tokens, 101);
    assert!(!tokens.live.contains_key(&oldest_live));
}

/// A run of flow questions nobody answers is bounded like every other kind:
/// the oldest is evicted, and other kinds keep their own slots.
#[test]
fn flow_ask_tokens_evict_their_own_oldest() {
    let mut tokens = CardTokens::<String>::default();
    let (approve, _) = tokens.mint_approval(ApprovalId(1));
    let first = flow_ask(&mut tokens, 0).remove(0);
    let second = flow_ask(&mut tokens, 1).remove(0);
    for ask_id in 2..=TOKENS_PER_KIND_CAP as u64 {
        flow_ask(&mut tokens, ask_id);
    }
    assert_eq!(tokens.route_tap(&first), Some(InboundAction::Ignore));
    assert!(matches!(
        tokens.route_tap(&second),
        Some(InboundAction::ResolveFlowAsk { ask_id: 1, .. })
    ));
    assert!(matches!(
        tokens.route_tap(&approve),
        Some(InboundAction::ResolveApproval { .. })
    ));
    tokens.forget(&flow_card(1));
    flow_ask(&mut tokens, 100);
    assert!(
        !tokens.live.contains_key(&second),
        "forgotten with its card"
    );
    assert_eq!(
        tokens.order[&Kind::FlowAsk].len(),
        TOKENS_PER_KIND_CAP,
        "a forgotten card frees its slot"
    );
}

fn question(tokens: &mut CardTokens<String>, step: u64) -> (QuestionPromptRef, Vec<String>) {
    let question = QuestionPromptRef {
        session: 1,
        question_id: 4,
        field: 0,
    };
    let message = tokens.mint_ping(BridgePing {
        pane: pane(1),
        header: "question".into(),
        tail: MessageTail::Plain("which?".into()),
        prompt: Some(PingPrompt::Question(QuestionPrompt {
            question,
            step,
            rows: vec![
                vec![("A".into(), QuestionButton::Option(0))],
                vec![("Skip".into(), QuestionButton::Skip)],
            ],
        })),
        weight: crate::remote_channel::bridge::PingWeight::Progress,
    });
    (question, tokens_of(message))
}

fn question_card(question: QuestionPromptRef, step: u64) -> CardId {
    CardId::Question {
        pane: pane(1),
        question,
        step,
    }
}

/// Text replying to a live question card answers that field. Once the card
/// is settled the reply is stale; a later step's card is still live.
#[test]
fn a_reply_names_its_card_until_that_card_settles() {
    let mut tokens = CardTokens::<String>::default();
    let (asked, _) = question(&mut tokens, 9);
    tokens.note_question_sent("m1".into(), pane(1), asked, 9);
    let live = Some(QuestionCard::Live {
        pane: pane(1),
        question: asked,
    });
    assert_eq!(tokens.question_reply(Some(&"m1".into())), live);
    tokens.note_question_sent("m2".into(), pane(1), asked, 10);
    tokens.tombstone(&question_card(asked, 9));
    assert_eq!(
        tokens.question_reply(Some(&"m1".into())),
        Some(QuestionCard::Settled)
    );
    assert_eq!(tokens.question_reply(Some(&"m2".into())), live);
    assert_eq!(tokens.question_reply(Some(&"m3".into())), None);
    assert_eq!(tokens.question_reply(None), None);
}

#[test]
fn a_question_button_answers_its_field_once() {
    let mut tokens = CardTokens::<String>::default();
    let (asked, buttons) = question(&mut tokens, 9);
    assert_eq!(
        tokens.route_tap(&buttons[0]),
        Some(InboundAction::RespondQuestion {
            pane: pane(1),
            question: asked,
            reply: QuestionReply::Button(QuestionButton::Option(0)),
        })
    );
    assert_eq!(tokens.route_tap(&buttons[0]), Some(InboundAction::Ignore));
    tokens.tombstone(&question_card(asked, 9));
    assert_eq!(tokens.route_tap(&buttons[1]), Some(InboundAction::Ignore));
    assert!(tokens.is_settled(&buttons[1]));
}

/// Forgetting one card leaves every other card's tokens live.
#[test]
fn forgetting_a_card_touches_only_its_own_tokens() {
    let mut tokens = CardTokens::<String>::default();
    let (approve, refuse) = tokens.mint_approval(ApprovalId(1));
    let (other, _) = tokens.mint_approval(ApprovalId(2));
    let flow = flow_ask(&mut tokens, 3).remove(0);
    tokens.forget(&CardId::Approval(ApprovalId(1)));
    assert_eq!(
        tokens.route_tap(&approve),
        Some(InboundAction::StaleApproval)
    );
    assert_eq!(
        tokens.route_tap(&refuse),
        Some(InboundAction::StaleApproval)
    );
    assert!(!tokens.is_settled(&approve));
    assert!(matches!(
        tokens.route_tap(&other),
        Some(InboundAction::ResolveApproval {
            id: ApprovalId(2),
            ..
        })
    ));
    assert!(matches!(
        tokens.route_tap(&flow),
        Some(InboundAction::ResolveFlowAsk { ask_id: 3, .. })
    ));
}

/// A reply in a pane's thread names the pane, not a card: it answers the
/// newest card that pane still waits on, and nothing once all are settled.
#[test]
fn a_thread_reply_answers_the_panes_newest_live_question() {
    let mut tokens = CardTokens::<String>::default();
    let (asked, _) = question(&mut tokens, 9);
    tokens.note_question_sent("m1".into(), pane(1), asked, 9);
    tokens.note_question_sent("m2".into(), pane(1), asked, 10);
    let live = Some(QuestionCard::Live {
        pane: pane(1),
        question: asked,
    });
    assert_eq!(tokens.live_question_of(pane(1)), live);
    assert_eq!(tokens.live_question_of(pane(2)), None, "another pane");
    tokens.tombstone(&question_card(asked, 10));
    assert_eq!(
        tokens.live_question_of(pane(1)),
        live,
        "the older step is live"
    );
    tokens.tombstone(&question_card(asked, 9));
    assert_eq!(tokens.live_question_of(pane(1)), None, "all settled");
}
