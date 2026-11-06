use super::*;
use crate::control::approval::ApprovalId;
use crate::remote_channel::cards::{CardCompletion, CardId, SendClaim, SendOutcome, Settlement};
use std::time::{Duration, Instant};

const CANCELLED: &str = "Cancelled at quit";

fn approval(id: u64) -> CardId {
    CardId::Approval(ApprovalId(id))
}

/// A card at every send state, and one settlement edit still waiting.
fn every_state() -> (OwnedCards<&'static str, &'static str, i64>, SendClaim) {
    let mut owned = OwnedCards::default();
    owned.bind("bot");
    let ledger = owned.ledger_mut();
    let sent = ledger.claim(approval(1)).unwrap();
    assert!(ledger.begin(&sent));
    ledger.complete(&sent, SendOutcome::Delivered("sent"));
    let sending = ledger.claim(approval(2)).unwrap();
    assert!(ledger.begin(&sending));
    let attempted = ledger.claim(approval(3)).unwrap();
    assert!(ledger.begin(&attempted));
    ledger.complete(&attempted, SendOutcome::Unconfirmed);
    ledger.claim(approval(4)).unwrap();
    owned.edits.push("settled", "Allowed".into());
    (owned, sending)
}

#[test]
fn quitting_cancels_sent_cards_and_keeps_queued_settlements() {
    let (mut owned, _) = every_state();
    let mut edits = owned.close_for_quit(CANCELLED);
    edits.sort();
    assert_eq!(
        edits,
        [
            ("sent", CANCELLED.to_owned()),
            ("settled", "Allowed".into())
        ]
    );
}

#[test]
fn cards_without_a_receipt_are_left_for_tap_time_cleanup() {
    let (mut owned, sending) = every_state();
    owned.close_for_quit(CANCELLED);
    let ledger = owned.ledger_mut();
    for card in [approval(2), approval(3), approval(4)] {
        assert!(ledger.contains(&card));
    }
    assert_eq!(
        ledger.complete(&sending, SendOutcome::Delivered("late")),
        CardCompletion::Delivered
    );
}

#[test]
fn a_card_closed_at_quit_is_never_edited_or_offered_again() {
    let (mut owned, _) = every_state();
    owned.close_for_quit(CANCELLED);
    assert!(owned.close_for_quit(CANCELLED).is_empty());
    assert!(owned.edits.claim().is_empty());
    let ledger = owned.ledger_mut();
    assert_eq!(
        ledger.settle(&approval(1), "Allowed".into()),
        Settlement::Recorded
    );
    assert!(ledger.claim(approval(1)).is_none());
}

#[test]
fn every_edit_that_finishes_in_time_is_settled() {
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let edits = (0..3)
        .map(|_| {
            let ran = ran.clone();
            QuitEdit::new("test edit failed", "test.quit_edit", move || {
                ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        })
        .collect();
    let left = Settling::start(edits).wait(Instant::now() + Duration::from_secs(10));
    assert_eq!(left, 0);
    assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn an_edit_still_running_at_the_deadline_is_counted_left() {
    let (release, blocked) = std::sync::mpsc::channel::<()>();
    let edits = vec![
        QuitEdit::new("test edit failed", "test.quit_edit", || Ok(())),
        QuitEdit::new("test edit failed", "test.quit_edit", move || {
            // SILENT-OK: the test drops the sender to release this edit.
            let _ = blocked.recv();
            Ok(())
        }),
    ];
    let started = Instant::now();
    let left = Settling::start(edits).wait(started + Duration::from_millis(100));
    assert_eq!(left, 1);
    assert!(started.elapsed() < Duration::from_secs(2));
    drop(release);
}
