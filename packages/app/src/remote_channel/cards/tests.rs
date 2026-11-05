use super::*;

#[derive(Debug)]
pub(crate) struct CapturedEdit {
    pub channel: String,
    pub ids: Vec<String>,
    pub original: String,
    pub label: String,
}

#[derive(Default)]
pub(crate) struct EditCapture {
    pub edits: Vec<CapturedEdit>,
    /// Every captured edit answers with this failure while it is set.
    pub fail: Option<EditFailure>,
}

impl gpui::Global for EditCapture {}

pub(crate) fn capture(
    channel: &str,
    ids: Vec<String>,
    original: &str,
    label: &str,
    cx: &mut gpui::App,
) -> Option<Result<(), EditFailure>> {
    if !cx.has_global::<EditCapture>() {
        return None;
    }
    let capture = cx.global_mut::<EditCapture>();
    capture.edits.push(CapturedEdit {
        channel: channel.to_owned(),
        ids,
        original: original.to_owned(),
        label: label.to_owned(),
    });
    Some(capture.fail.clone().map_or(Ok(()), Err))
}

fn approval() -> CardId {
    CardId::Approval(crate::control::approval::ApprovalId(1))
}

#[test]
fn a_sent_card_is_consumed_once_with_its_original_receipt() {
    let mut cards = CardLedger::default();
    let claim = cards.claim(approval()).unwrap();
    assert!(cards.claim(approval()).is_none());
    assert!(cards.begin(&claim));
    assert_eq!(
        cards.complete(&claim, SendOutcome::Delivered("message-1")),
        CardCompletion::Delivered
    );
    assert_eq!(
        cards.settle(&approval(), "Allowed".into()),
        Settlement::Edit {
            receipt: "message-1",
            label: "Allowed".into()
        }
    );
    assert_eq!(
        cards.settle(&approval(), "Rejected".into()),
        Settlement::Recorded
    );
    assert!(!cards.contains(&approval()));
}

#[test]
fn settlement_while_sending_edits_the_late_receipt_with_the_first_result() {
    let mut cards = CardLedger::default();
    let claim = cards.claim(approval()).unwrap();
    assert!(cards.begin(&claim));
    assert_eq!(
        cards.settle(&approval(), "Cancelled".into()),
        Settlement::Recorded
    );
    assert_eq!(
        cards.settle(&approval(), "Allowed".into()),
        Settlement::Recorded
    );
    assert_eq!(
        cards.complete(&claim, SendOutcome::Delivered("late-id")),
        CardCompletion::Edit {
            receipt: "late-id",
            label: "Cancelled".into()
        }
    );
    assert!(!cards.contains(&approval()));
}

#[test]
fn settlement_before_preparation_cannot_register_live_buttons() {
    let mut cards = CardLedger::<String>::default();
    let claim = cards.claim(approval()).unwrap();
    assert_eq!(
        cards.settle(&approval(), "Cancelled".into()),
        Settlement::Recorded
    );
    assert!(!cards.begin(&claim));
    assert!(!cards.contains(&approval()));
}

#[test]
fn ambiguous_transport_failure_is_attempted_until_the_request_settles() {
    let mut cards = CardLedger::<String>::default();
    let claim = cards.claim(approval()).unwrap();
    assert!(cards.begin(&claim));
    assert_eq!(
        cards.complete(&claim, SendOutcome::Unconfirmed),
        CardCompletion::Unconfirmed
    );
    assert!(cards.contains(&approval()));
    assert!(cards.claim(approval()).is_none());
    assert_eq!(
        cards.settle(&approval(), "Cancelled".into()),
        Settlement::Unreceipted
    );
    assert!(!cards.contains(&approval()));
}

#[test]
fn a_send_that_never_left_releases_its_claim_for_another_offer() {
    let mut cards = CardLedger::<String>::default();
    let claim = cards.claim(approval()).unwrap();
    assert!(cards.begin(&claim));
    assert_eq!(
        cards.complete(&claim, SendOutcome::NotSent),
        CardCompletion::Released
    );
    assert!(!cards.contains(&approval()));
    let again = cards.claim(approval()).unwrap();
    assert!(cards.begin(&again));
    cards.settle(&approval(), "Cancelled".into());
    assert_eq!(
        cards.complete(&again, SendOutcome::NotSent),
        CardCompletion::Released
    );
    assert!(!cards.contains(&approval()));
}

#[test]
fn rejected_queue_releases_the_claim_and_old_completions_cannot_touch_a_new_claim() {
    let mut cards = CardLedger::default();
    let first = cards.claim(approval()).unwrap();
    cards.release(&first);
    let next = cards.claim(approval()).unwrap();
    assert!(cards.begin(&next));
    cards.release(&first);
    assert_eq!(
        cards.complete(&first, SendOutcome::Delivered("old")),
        CardCompletion::Ignored
    );
    assert_eq!(
        cards.complete(&next, SendOutcome::Delivered("new")),
        CardCompletion::Delivered
    );
    assert_eq!(
        cards.settle(&approval(), "Allowed".into()),
        Settlement::Edit {
            receipt: "new",
            label: "Allowed".into()
        }
    );
}

#[test]
fn sending_failure_after_settlement_retains_no_tombstone() {
    let mut cards = CardLedger::<String>::default();
    let claim = cards.claim(approval()).unwrap();
    assert!(cards.begin(&claim));
    cards.settle(&approval(), "Cancelled".into());
    assert_eq!(
        cards.complete(&claim, SendOutcome::Unconfirmed),
        CardCompletion::Unconfirmed
    );
    assert!(!cards.contains(&approval()));
}

#[test]
fn only_the_current_unsettled_attempt_is_sending() {
    let mut cards = CardLedger::default();
    let old = cards.claim(approval()).unwrap();
    assert!(!cards.is_sending(&old));
    assert!(cards.begin(&old));
    assert!(cards.is_sending(&old));
    assert!(cards.release(&old));
    let current = cards.claim(approval()).unwrap();
    assert!(cards.begin(&current));
    assert!(!cards.is_sending(&old));
    assert!(cards.is_sending(&current));
    cards.complete(&old, SendOutcome::Delivered("old"));
    assert!(cards.is_sending(&current));
    cards.complete(&current, SendOutcome::Delivered("current"));
    assert!(!cards.is_sending(&current));
    cards.settle(&approval(), "Allowed".into());
    assert!(!cards.is_sending(&current));
}

#[test]
fn duplicate_or_unprepared_completions_have_no_effect() {
    let mut cards = CardLedger::default();
    let claim = cards.claim(approval()).unwrap();
    assert_eq!(
        cards.complete(&claim, SendOutcome::Delivered("early")),
        CardCompletion::Ignored
    );
    assert!(cards.begin(&claim));
    assert_eq!(
        cards.complete(&claim, SendOutcome::Delivered("first")),
        CardCompletion::Delivered
    );
    assert_eq!(
        cards.complete(&claim, SendOutcome::Unconfirmed),
        CardCompletion::Ignored
    );
    assert_eq!(
        cards.complete(&claim, SendOutcome::Delivered("duplicate")),
        CardCompletion::Ignored
    );
    assert_eq!(
        cards.settle(&approval(), "Allowed".into()),
        Settlement::Edit {
            receipt: "first",
            label: "Allowed".into()
        }
    );
}

#[test]
fn rebinding_the_same_identity_keeps_records_and_another_returns_them() {
    let mut owned: OwnedCards<&str, &str, i64> = OwnedCards::default();
    assert!(owned.bind("bot-a").is_empty());
    let ledger = owned.ledger_for(&"bot-a").unwrap();
    let claim = ledger.claim(approval()).unwrap();
    assert!(ledger.begin(&claim));
    ledger.complete(&claim, SendOutcome::Delivered("message-1"));
    assert!(owned.bind("bot-a").is_empty(), "a reconnect keeps the card");
    assert!(owned.ledger().contains(&approval()));
    assert!(owned.ledger_for(&"bot-b").is_none());
    assert_eq!(owned.bind("bot-b"), vec![approval()]);
    assert!(!owned.ledger().contains(&approval()));
    assert!(
        owned
            .ledger_for(&"bot-b")
            .unwrap()
            .claim(approval())
            .is_some()
    );
}

#[test]
fn settled_messages_are_remembered_for_stale_taps_and_bounded() {
    let mut owned: OwnedCards<&str, &str, i64> = OwnedCards::default();
    owned.bind("bot");
    assert!(owned.untracked(&1));
    owned.note_settled(1);
    assert!(!owned.untracked(&1));
    for message in 2..=(SETTLED_MESSAGES_CAP as i64 + 1) {
        owned.note_settled(message);
    }
    assert!(owned.untracked(&1), "the oldest is evicted");
    assert!(!owned.untracked(&2));
    owned.bind("other");
    assert!(owned.untracked(&2), "a new identity starts clean");
}

#[test]
fn retiring_drops_records_and_the_identity() {
    let mut owned: OwnedCards<&str, &str, i64> = OwnedCards::default();
    owned.bind("bot");
    owned.ledger_for(&"bot").unwrap().claim(approval()).unwrap();
    assert_eq!(owned.retire(), vec![approval()]);
    assert!(owned.owner().is_none());
    assert!(owned.bind("bot").is_empty());
    assert!(!owned.ledger().contains(&approval()));
}

#[test]
fn queued_settlement_edits_follow_the_identity_and_not_a_connection() {
    let mut owned: OwnedCards<&str, &str, i64> = OwnedCards::default();
    owned.bind("bot");
    owned.edits.push("message", "Approved".into());
    owned.bind("bot");
    let (id, _, _) = owned.edits.claim().pop().unwrap();
    owned.edits.finish(id, EditOutcome::Transient);
    owned.bind("replacement");
    assert!(owned.edits.claim().is_empty());
    owned.edits.push("replacement-message", "Refused".into());
    owned.retire();
    assert!(owned.edits.claim().is_empty());
}
