use super::super::publish_queue::{PublishQueue, Settled, Viewer};
use super::*;
use crate::remote_channel::transport::slack::home::PublishVerdict;

fn sent(queue: &mut PublishQueue) -> Vec<Vec<String>> {
    queue
        .ready()
        .into_iter()
        .map(|publication| {
            publication
                .board
                .rows
                .iter()
                .map(|row| row.name.clone())
                .collect()
        })
        .collect()
}

/// Opened and wanted: one publish of the latest board, and the same board
/// is not sent again once shown.
#[test]
fn an_opened_page_gets_the_latest_board_once() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.want(&work, board(&["early"]));
    assert!(
        queue.ready().is_empty(),
        "not published before it is opened"
    );
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    queue.want(&work, board(&["b"]));
    let first = queue.ready();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].board, board(&["b"]), "the latest board wins");
    queue.want(&work, board(&["c"]));
    assert!(queue.ready().is_empty(), "one publish at a time");
    assert_eq!(
        queue.finish(&first[0], &PublishVerdict::Published),
        Settled::Published
    );
    let second = queue.ready();
    assert_eq!(second[0].board, board(&["c"]), "what changed meanwhile");
    queue.finish(&second[0], &PublishVerdict::Published);
    queue.want(&work, board(&["c"]));
    assert!(queue.ready().is_empty(), "an identical board is skipped");
}

#[test]
fn each_verdict_leaves_the_page_as_it_says() {
    let cases = [
        (
            PublishVerdict::Disabled,
            Settled::Unavailable { first: true },
            Viewer::Unavailable,
            false,
            "a page turned off stops until opened again",
        ),
        (
            PublishVerdict::Transient("ratelimited".into()),
            Settled::Retry,
            Viewer::Viewing,
            true,
            "passing trouble waits for the next opening",
        ),
        (
            PublishVerdict::Refused("invalid_blocks".into()),
            Settled::Dropped,
            Viewer::Viewing,
            false,
            "a refused board is dropped",
        ),
    ];
    for (verdict, settled, viewer, resent, label) in cases {
        let work = scope("work", "bot");
        let mut queue = PublishQueue::default();
        queue.opened(&work);
        queue.want(&work, board(&["a"]));
        let ready = queue.ready();
        assert_eq!(queue.finish(&ready[0], &verdict), settled, "{label}");
        assert_eq!(queue.viewer(&work), Some(viewer), "{label}");
        assert!(queue.ready().is_empty(), "{label}: nothing sent unasked");
        queue.opened(&work);
        assert_eq!(
            !sent(&mut queue).is_empty(),
            resent,
            "{label}: what the next opening sends by itself"
        );
    }
}

#[test]
fn a_turned_off_page_is_reported_once_per_connection() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    for first in [true, false] {
        queue.opened(&work);
        queue.want(&work, board(&["a"]));
        let ready = queue.ready();
        assert_eq!(
            queue.finish(&ready[0], &PublishVerdict::Disabled),
            Settled::Unavailable { first }
        );
    }
}

/// Another bot or conversation serving the connection starts a page of its
/// own: the old one's state and results do not carry over.
#[test]
fn another_identity_starts_a_new_page() {
    let old = scope("work", "bot");
    let new = scope("work", "other");
    let mut queue = PublishQueue::default();
    queue.opened(&old);
    queue.want(&old, board(&["a"]));
    let in_flight = queue.ready();
    queue.want(&new, board(&["b"]));
    assert_eq!(queue.viewer(&new), None, "not opened under the new bot");
    queue.opened(&new);
    assert_eq!(queue.viewer(&old), None, "the old page is forgotten");
    assert_eq!(
        queue.finish(&in_flight[0], &PublishVerdict::Published),
        Settled::Stale
    );
    queue.want(&new, board(&["b"]));
    assert_eq!(sent(&mut queue), [["b"]]);
}

/// A connection that cannot send keeps its board for the next opening.
#[test]
fn a_held_board_waits_for_the_next_opening() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    let ready = queue.ready();
    queue.hold(&ready[0]);
    assert!(queue.ready().is_empty());
    queue.opened(&work);
    assert_eq!(sent(&mut queue), [["a"]]);
}
