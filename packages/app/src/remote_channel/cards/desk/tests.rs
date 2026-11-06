//! A card's ledger state and its callback tokens change together. Each case
//! drives one card through the steps a transport takes and asks what a tap
//! or a reply on it routes to afterwards.

use super::*;
use crate::control::approval::{ApprovalChoice, ApprovalId};
use crate::control::spec::Ordinal;
use crate::remote_channel::bridge::{
    BridgePing, FlowAskPrompt, InboundAction, InlineKeyboard, MessageTail, Outbound,
    PermissionDecision, PermissionPromptRef, PingPrompt, QuestionButton, QuestionPrompt,
    QuestionPromptRef, QuestionReply, Routed, RoutingCore,
};
use crate::remote_channel::cards::{CardCompletion, CardId, SendClaim, SendOutcome, Settlement};
use daruda_store::project::WorkspaceUuid;

/// One identity's cards as a transport holds them, with its routing core for
/// what a tap or reply reaches outside the cards.
#[derive(Default)]
struct Harness {
    routing: RoutingCore<String>,
    desk: CardDesk<String, &'static str, &'static str>,
}

impl Harness {
    fn bound(owner: &'static str) -> Self {
        let mut harness = Self::default();
        harness.bind(owner);
        harness
    }

    fn bind(&mut self, owner: &'static str) {
        self.desk.bind(owner);
    }

    fn retire(&mut self) {
        self.desk.retire();
    }

    fn claim(&mut self, card: CardId) -> Option<SendClaim> {
        self.desk.claim(card)
    }

    fn claim_approval(&mut self, id: ApprovalId) -> Option<(SendClaim, (String, String))> {
        self.desk.claim_approval(id)
    }

    fn abandon(&mut self, claim: &SendClaim) -> bool {
        self.desk.abandon(claim)
    }

    /// The callback tokens on the message `begin` prepared.
    fn begin(&mut self, claim: Option<&SendClaim>, outbound: Outbound) -> Option<Vec<String>> {
        let keyboard = match self.desk.begin(claim, outbound)? {
            Begun::Ping { message, .. } | Begun::FlowAsk(message) => message.keyboard,
            Begun::Approval(prompt) => Some(InlineKeyboard::single_row(prompt.buttons.to_vec())),
            Begun::Notice(_) => None,
        };
        Some(
            keyboard
                .map(|keyboard| {
                    keyboard
                        .rows
                        .into_iter()
                        .flatten()
                        .map(|(_, token)| token)
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    fn complete(
        &mut self,
        claim: &SendClaim,
        outcome: SendOutcome<&'static str>,
        replies: Vec<String>,
    ) -> CardCompletion<&'static str> {
        self.desk.complete(claim, outcome, replies)
    }

    fn settle(&mut self, card: &CardId, label: &str) -> Settlement<&'static str> {
        self.desk.settle(card, label.to_owned())
    }

    fn tap(&mut self, data: &str) -> InboundAction {
        self.desk
            .route_tap(data)
            .unwrap_or_else(|| self.routing.route_listing(data))
    }

    fn reply(&mut self, text: &str, reply_to: &str) -> Routed {
        let reply_to = reply_to.to_owned();
        let card = self.desk.question_reply(Some(&reply_to));
        self.routing
            .route_text(text.to_owned(), Some(reply_to), card)
    }

    fn note_tap(&mut self, data: &str, message: &str, managed: bool) {
        self.desk.note_tap(data, message.to_owned(), managed);
    }

    fn untracked(&self, message: &str) -> bool {
        self.desk.untracked(&message.to_owned())
    }

    fn open_permissions(&self, pane: PaneRef, session: u64) -> Vec<CardId> {
        self.desk.open(CardScope::Permissions { pane, session })
    }

    fn open_questions(&self, pane: PaneRef, session: u64, question_id: Option<u64>) -> Vec<CardId> {
        self.desk.open(CardScope::Questions {
            pane,
            session,
            question_id,
        })
    }
}

fn pane(n: u64) -> PaneRef {
    PaneRef {
        workspace: WorkspaceUuid::default(),
        pane: n,
    }
}

fn permission_ping(request: u64) -> (CardId, Outbound) {
    let ping = BridgePing {
        pane: pane(1),
        header: "permission".into(),
        tail: MessageTail::Plain("write".into()),
        prompt: Some(PingPrompt::Permission(PermissionPromptRef {
            session: 1,
            perm_id: request,
            buttons: vec![
                ("Allow".into(), PermissionDecision::Allow("once".into())),
                ("Reject".into(), PermissionDecision::Reject("no".into())),
            ],
        })),
    };
    let outbound = Outbound::Ping(ping);
    (outbound.card_id().unwrap(), outbound)
}

const QUESTION: QuestionPromptRef = QuestionPromptRef {
    session: 1,
    question_id: 4,
    field: 0,
};

fn question_ping(step: u64) -> (CardId, Outbound) {
    let outbound = Outbound::Ping(BridgePing {
        pane: pane(1),
        header: "question".into(),
        tail: MessageTail::Plain("which?".into()),
        prompt: Some(PingPrompt::Question(QuestionPrompt {
            question: QUESTION,
            step,
            rows: vec![
                vec![("A".into(), QuestionButton::Option(0))],
                vec![("Skip".into(), QuestionButton::Skip)],
            ],
        })),
    });
    (outbound.card_id().unwrap(), outbound)
}

fn flow_ask(ask_id: u64) -> (CardId, Outbound) {
    let outbound = Outbound::FlowAsk(FlowAskPrompt {
        worktree: "/tree".into(),
        run_dir: "/tree/run".into(),
        ask_id,
        summary: "Question".into(),
        buttons: vec![("Allow".into(), PermissionDecision::Allow("once".into()))],
    });
    (outbound.card_id().unwrap(), outbound)
}

fn resolve(choice: ApprovalChoice) -> InboundAction {
    InboundAction::ResolveApproval {
        id: ApprovalId(1),
        choice,
    }
}

/// Queue, prepare and deliver an approval card; its two tokens.
fn delivered_approval(harness: &mut Harness) -> (SendClaim, String, String) {
    let (claim, (approve, refuse)) = harness.claim_approval(ApprovalId(1)).unwrap();
    assert!(
        harness
            .begin(Some(&claim), Outbound::Notice(String::new()))
            .is_some()
    );
    assert_eq!(
        harness.complete(&claim, SendOutcome::Delivered("m1"), Vec::new()),
        CardCompletion::Delivered
    );
    (claim, approve, refuse)
}

/// Queue and prepare a card, leaving it sending; its tokens.
fn sending(
    harness: &mut Harness,
    (card, outbound): (CardId, Outbound),
) -> (SendClaim, Vec<String>) {
    let claim = harness.claim(card).unwrap();
    let tokens = harness.begin(Some(&claim), outbound).unwrap();
    (claim, tokens)
}

#[test]
fn a_delivered_card_keeps_live_tokens_until_settlement_tombstones_them() {
    let mut harness = Harness::bound("bot");
    let (_, approve, refuse) = delivered_approval(&mut harness);
    assert_eq!(harness.tap(&approve), resolve(ApprovalChoice::Approved));
    assert_eq!(
        harness.tap(&approve),
        resolve(ApprovalChoice::Approved),
        "an approval tap is not consumed"
    );
    assert_eq!(
        harness.settle(&CardId::Approval(ApprovalId(1)), "Allowed"),
        Settlement::Edit {
            receipt: "m1",
            label: "Allowed".into()
        }
    );
    assert_eq!(harness.tap(&refuse), InboundAction::StaleApproval);
    harness.note_tap(&refuse, "m1", false);
    assert!(
        !harness.untracked("m1"),
        "a settled token names a settled message"
    );

    let (card, outbound) = permission_ping(7);
    let (claim, tokens) = sending(&mut harness, (card.clone(), outbound));
    harness.complete(&claim, SendOutcome::Delivered("m2"), Vec::new());
    assert!(matches!(
        harness.tap(&tokens[0]),
        InboundAction::RespondPermission { perm_id: 7, .. }
    ));
    assert_eq!(harness.tap(&tokens[0]), InboundAction::Ignore, "consumed");
    assert!(matches!(
        harness.settle(&card, "Allowed"),
        Settlement::Edit { .. }
    ));
    assert_eq!(harness.tap(&tokens[1]), InboundAction::Ignore);
    harness.note_tap(&tokens[1], "m2", false);
    assert!(!harness.untracked("m2"));
}

#[test]
fn settling_an_unreceipted_card_forgets_its_tokens_without_a_tombstone() {
    let mut harness = Harness::bound("bot");
    let (card, outbound) = permission_ping(7);
    let (claim, tokens) = sending(&mut harness, (card.clone(), outbound));
    assert_eq!(
        harness.complete(&claim, SendOutcome::Unconfirmed, Vec::new()),
        CardCompletion::Unconfirmed
    );
    assert!(
        matches!(
            harness.tap(&tokens[0]),
            InboundAction::RespondPermission { .. }
        ),
        "an unconfirmed card may be on screen, so its buttons still answer"
    );
    assert_eq!(harness.settle(&card, "Cancelled"), Settlement::Unreceipted);
    assert_eq!(harness.tap(&tokens[1]), InboundAction::Ignore);
    harness.note_tap(&tokens[1], "m", false);
    assert!(
        harness.untracked("m"),
        "no edit owns it, so a tap strips it"
    );

    let (claim, (approve, _)) = harness.claim_approval(ApprovalId(1)).unwrap();
    harness.begin(Some(&claim), Outbound::Notice(String::new()));
    harness.complete(&claim, SendOutcome::Unconfirmed, Vec::new());
    assert_eq!(
        harness.settle(&CardId::Approval(ApprovalId(1)), "Cancelled"),
        Settlement::Unreceipted
    );
    assert_eq!(harness.tap(&approve), InboundAction::StaleApproval);
    harness.note_tap(&approve, "m", false);
    assert!(harness.untracked("m"));
}

#[test]
fn settling_while_sending_tombstones_and_the_late_receipt_is_edited() {
    let mut harness = Harness::bound("bot");
    let (card, outbound) = flow_ask(3);
    let (claim, tokens) = sending(&mut harness, (card.clone(), outbound));
    assert_eq!(harness.settle(&card, "Done"), Settlement::Recorded);
    assert!(
        matches!(
            harness.tap(&tokens[0]),
            InboundAction::ResolveFlowAsk { ask_id: 3, .. }
        ),
        "a late flow tap still names its question"
    );
    harness.note_tap(&tokens[0], "m", false);
    assert!(!harness.untracked("m"));
    assert_eq!(
        harness.complete(&claim, SendOutcome::Delivered("m"), Vec::new()),
        CardCompletion::Edit {
            receipt: "m",
            label: "Done".into()
        }
    );
    assert!(harness.claim(card).is_some(), "the edit retired the card");
}

#[test]
fn settling_a_card_with_no_record_still_retires_its_question() {
    let mut harness = Harness::bound("a");
    let (card, outbound) = question_ping(9);
    let (claim, _) = sending(&mut harness, (card.clone(), outbound));
    harness.complete(&claim, SendOutcome::Delivered("m1"), vec!["m1".into()]);
    harness.bind("b");
    assert!(
        matches!(
            harness.reply("teal", "m1"),
            Routed::Ready(InboundAction::RespondQuestion { .. })
        ),
        "a reply needs no token, so it outlives the identity's records"
    );
    assert_eq!(harness.settle(&card, "Answered"), Settlement::Recorded);
    assert_eq!(
        harness.reply("late", "m1"),
        Routed::Ready(InboundAction::StaleQuestion)
    );
}

#[test]
fn a_released_send_forgets_its_tokens_and_may_be_offered_again() {
    let mut harness = Harness::bound("bot");
    let (claim, (approve, _)) = harness.claim_approval(ApprovalId(1)).unwrap();
    harness.begin(Some(&claim), Outbound::Notice(String::new()));
    assert_eq!(
        harness.complete(&claim, SendOutcome::NotSent, Vec::new()),
        CardCompletion::Released
    );
    assert_eq!(harness.tap(&approve), InboundAction::StaleApproval);
    harness.note_tap(&approve, "m", false);
    assert!(harness.untracked("m"), "released, not settled");

    let (first, (approve, _)) = harness.claim_approval(ApprovalId(1)).unwrap();
    assert!(harness.abandon(&first));
    assert_eq!(harness.tap(&approve), InboundAction::StaleApproval);
    assert!(!harness.abandon(&first));

    let (_, (approve, _)) = harness.claim_approval(ApprovalId(1)).unwrap();
    assert!(
        !harness.abandon(&first),
        "an old claim cannot release the current one"
    );
    assert_eq!(harness.tap(&approve), resolve(ApprovalChoice::Approved));
}

#[test]
fn a_settlement_before_preparation_leaves_nothing_to_tap() {
    let mut harness = Harness::bound("bot");
    let (card, outbound) = permission_ping(7);
    let claim = harness.claim(card.clone()).unwrap();
    assert_eq!(harness.settle(&card, "Cancelled"), Settlement::Recorded);
    assert!(harness.begin(Some(&claim), outbound).is_none());
    assert!(harness.claim(card).is_some());
}

#[test]
fn rebinding_the_same_owner_keeps_tokens_and_another_drops_them() {
    let mut harness = Harness::bound("a");
    let (_, approve, _) = delivered_approval(&mut harness);
    let (card, outbound) = flow_ask(3);
    let (_, flow) = sending(&mut harness, (card.clone(), outbound));
    harness.settle(&card, "Done");
    harness.bind("a");
    assert_eq!(harness.tap(&approve), resolve(ApprovalChoice::Approved));
    assert!(matches!(
        harness.tap(&flow[0]),
        InboundAction::ResolveFlowAsk { .. }
    ));
    harness.bind("b");
    assert_eq!(harness.tap(&approve), InboundAction::StaleApproval);
    assert_eq!(
        harness.tap(&flow[0]),
        InboundAction::Ignore,
        "tombstones go too"
    );
    assert!(harness.claim_approval(ApprovalId(1)).is_some());
}

#[test]
fn retiring_forgets_every_token_and_tombstone() {
    let mut harness = Harness::bound("a");
    let (_, approve, _) = delivered_approval(&mut harness);
    let (card, outbound) = flow_ask(3);
    let (_, flow) = sending(&mut harness, (card.clone(), outbound));
    harness.settle(&card, "Done");
    harness.retire();
    assert_eq!(harness.tap(&approve), InboundAction::StaleApproval);
    assert_eq!(harness.tap(&flow[0]), InboundAction::Ignore);
    assert!(harness.claim_approval(ApprovalId(1)).is_some());
}

#[test]
fn a_reply_answers_a_delivered_question_card_until_it_settles() {
    let mut harness = Harness::bound("bot");
    let (card, outbound) = question_ping(9);
    let (claim, tokens) = sending(&mut harness, (card.clone(), outbound));
    harness.complete(&claim, SendOutcome::Delivered("m1"), vec!["m1".into()]);
    assert_eq!(
        harness.reply("teal", "m1"),
        Routed::Ready(InboundAction::RespondQuestion {
            pane: pane(1),
            question: QUESTION,
            reply: QuestionReply::Text("teal".into()),
        })
    );
    assert_eq!(
        harness.tap(&tokens[0]),
        InboundAction::RespondQuestion {
            pane: pane(1),
            question: QUESTION,
            reply: QuestionReply::Button(QuestionButton::Option(0)),
        }
    );
    assert_eq!(harness.tap(&tokens[0]), InboundAction::Ignore, "consumed");
    assert!(matches!(
        harness.settle(&card, "Answered"),
        Settlement::Edit { .. }
    ));
    assert_eq!(
        harness.reply("late", "m1"),
        Routed::Ready(InboundAction::StaleQuestion)
    );
    assert_eq!(harness.tap(&tokens[1]), InboundAction::Ignore);

    let (card, outbound) = question_ping(10);
    let (claim, _) = sending(&mut harness, (card, outbound));
    harness.complete(&claim, SendOutcome::Delivered("m2"), Vec::new());
    assert!(
        matches!(harness.reply("x", "m2"), Routed::NeedsTarget(_)),
        "a card delivered to a replaced session takes no reply"
    );
}

#[test]
fn an_unknown_token_is_answered_by_its_namespace() {
    let mut harness = Harness::bound("bot");
    harness.routing.command_state_mut().record_listing(
        &crate::remote_channel::command::tests::listing_of(&[pane(5)]),
    );
    let listing = harness
        .routing
        .command_state_mut()
        .listing_token(Ordinal(1))
        .unwrap();
    assert_eq!(
        harness.tap(&listing),
        InboundAction::SelectTarget { pane: pane(5) }
    );
    assert_eq!(harness.tap("lst:999:0"), InboundAction::StaleListing);
    assert_eq!(harness.tap("apv:gone"), InboundAction::StaleApproval);
    assert_eq!(harness.tap("qst:gone"), InboundAction::Ignore);
    assert_eq!(harness.tap("faq:gone"), InboundAction::Ignore);
    assert_eq!(harness.tap("0123456789abcdef"), InboundAction::Ignore);
}

#[test]
fn a_tap_is_settled_when_its_card_is_managed_or_its_token_retired() {
    let mut harness = Harness::bound("bot");
    harness.note_tap("unknown", "m1", false);
    assert!(harness.untracked("m1"));
    harness.note_tap("unknown", "m1", true);
    assert!(!harness.untracked("m1"));
}

#[test]
fn open_cards_are_found_by_scope() {
    let mut harness = Harness::bound("bot");
    let (permission, outbound) = permission_ping(7);
    sending(&mut harness, (permission.clone(), outbound));
    let (question, outbound) = question_ping(9);
    sending(&mut harness, (question.clone(), outbound));
    assert_eq!(harness.open_permissions(pane(1), 1), vec![permission]);
    assert!(harness.open_permissions(pane(1), 2).is_empty());
    assert_eq!(
        harness.open_questions(pane(1), 1, Some(4)),
        vec![question.clone()]
    );
    assert_eq!(harness.open_questions(pane(1), 1, None), vec![question]);
    assert!(harness.open_questions(pane(1), 1, Some(5)).is_empty());
}
