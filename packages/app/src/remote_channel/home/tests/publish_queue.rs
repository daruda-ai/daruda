use super::super::publish_queue::{Publication, PublishQueue, Settled, Viewer};
use super::*;
use crate::remote_channel::transport::slack::home::PublishVerdict;

fn sent(queue: &mut PublishQueue) -> Vec<Vec<String>> {
    queue
        .ready(false)
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
        queue.ready(false).is_empty(),
        "not published before it is opened"
    );
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    queue.want(&work, board(&["b"]));
    let first = queue.ready(false);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].board, board(&["b"]), "the latest board wins");
    queue.want(&work, board(&["c"]));
    assert!(queue.ready(false).is_empty(), "one publish at a time");
    assert_eq!(
        queue.finish(&first[0], &PublishVerdict::Published),
        Settled::Published
    );
    let second = queue.ready(false);
    assert_eq!(second[0].board, board(&["c"]), "what changed meanwhile");
    queue.finish(&second[0], &PublishVerdict::Published);
    queue.want(&work, board(&["c"]));
    assert!(
        queue.ready(false).is_empty(),
        "an identical board is skipped"
    );
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
        let ready = queue.ready(false);
        assert_eq!(queue.finish(&ready[0], &verdict), settled, "{label}");
        assert_eq!(queue.viewer(&work), Some(viewer), "{label}");
        assert!(
            queue.ready(false).is_empty(),
            "{label}: nothing sent unasked"
        );
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
        let ready = queue.ready(false);
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
    let in_flight = queue.ready(false);
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
    let ready = queue.ready(false);
    queue.hold(&ready[0]);
    assert!(queue.ready(false).is_empty());
    queue.opened(&work);
    assert_eq!(sent(&mut queue), [["a"]]);
}

/// Only a page the person opened is kept up to date as panes change: one
/// never opened, or turned off, gets nothing unasked.
#[test]
fn only_a_viewed_page_takes_a_new_board() {
    let opened = |queue: &mut PublishQueue| queue.opened(&scope("work", "bot"));
    let turned_off = |queue: &mut PublishQueue| {
        queue.opened(&scope("work", "bot"));
        queue.want(&scope("work", "bot"), board(&["a"]));
        let ready = queue.ready(false);
        queue.finish(&ready[0], &PublishVerdict::Disabled);
    };
    type Arrange = fn(&mut PublishQueue);
    let cases: [(&str, Arrange, bool); 3] = [
        ("viewing", opened, true),
        ("never opened", |_| {}, false),
        ("turned off", turned_off, false),
    ];
    for (label, arrange, sent_now) in cases {
        let work = scope("work", "bot");
        let mut queue = PublishQueue::default();
        arrange(&mut queue);
        assert_eq!(queue.viewing().next().is_some(), sent_now, "{label}");
        queue.want(&work, board(&["changed"]));
        assert_eq!(!sent(&mut queue).is_empty(), sent_now, "{label}");
    }
}

/// A board that failed for a passing reason, or whose connection could not
/// send, goes again on the retry tick, not on an ordinary flush.
#[test]
fn a_waiting_board_is_resent_on_the_retry_tick() {
    let transient = |queue: &mut PublishQueue, publication: &Publication| {
        queue.finish(
            publication,
            &PublishVerdict::Transient("ratelimited".into()),
        );
    };
    let held = |queue: &mut PublishQueue, publication: &Publication| queue.hold(publication);
    type Wait = fn(&mut PublishQueue, &Publication);
    let cases: [(&str, Wait); 2] = [("passing failure", transient), ("cannot send", held)];
    for (label, wait) in cases {
        let work = scope("work", "bot");
        let mut queue = PublishQueue::default();
        queue.opened(&work);
        queue.want(&work, board(&["a"]));
        let ready = queue.ready(false);
        wait(&mut queue, &ready[0]);
        assert!(queue.ready(false).is_empty(), "{label}: not on a flush");
        let retried = queue.ready(true);
        assert_eq!(retried.len(), 1, "{label}: on the retry tick");
        assert_eq!(retried[0].board, board(&["a"]), "{label}");
    }
}

/// The retry tick forgets pages whose identity no connection serves; the
/// others keep their viewer.
#[test]
fn a_page_no_connection_serves_is_forgotten() {
    let gone = scope("old", "bot");
    let kept = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.opened(&gone);
    queue.opened(&kept);
    queue.want(&gone, board(&["a"]));
    queue.keep_scopes(|scope| *scope == kept);
    assert_eq!(queue.viewer(&gone), None, "forgotten");
    assert_eq!(queue.viewer(&kept), Some(Viewer::Viewing), "kept");
    assert!(
        queue.ready(true).is_empty(),
        "nothing sent for the gone page"
    );
}
