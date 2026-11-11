use super::super::publish_queue::{Publication, PublishQueue, Settled, Viewer};
use super::*;
use crate::remote_channel::transport::PageVerdict;

fn sent(queue: &mut PublishQueue) -> Vec<Vec<String>> {
    queue
        .ready(false)
        .into_iter()
        .map(|publication| {
            publication
                .board
                .rows()
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
        queue.finish(&first[0], &PageVerdict::Published),
        Settled::Published
    );
    let second = queue.ready(false);
    assert_eq!(second[0].board, board(&["c"]), "what changed meanwhile");
    queue.finish(&second[0], &PageVerdict::Published);
    queue.want(&work, board(&["c"]));
    assert!(
        queue.ready(false).is_empty(),
        "an identical board is skipped"
    );
}

/// The defect this guards: a board equal to the published one was dropped
/// while another was in flight, so the page kept the in-flight board once it
/// landed. What is wanted is compared with what the page shows afterwards.
#[test]
fn a_board_wanted_during_a_publish_is_compared_after_it_lands() {
    let cases = [
        ("p", true, "back to what was published"),
        ("a", false, "the board already in flight"),
    ];
    for (later, resent, label) in cases {
        let work = scope("work", "bot");
        let mut queue = PublishQueue::default();
        queue.opened(&work);
        queue.want(&work, board(&["p"]));
        let first = queue.ready(false);
        queue.finish(&first[0], &PageVerdict::Published);
        queue.want(&work, board(&["a"]));
        let in_flight = queue.ready(false);
        assert_eq!(in_flight[0].board, board(&["a"]), "{label}");
        queue.want(&work, board(&[later]));
        assert!(queue.ready(false).is_empty(), "{label}: one at a time");
        queue.finish(&in_flight[0], &PageVerdict::Published);
        let want: &[[&str; 1]] = if resent { &[[later]] } else { &[] };
        assert_eq!(sent(&mut queue), want, "{label}");
    }
}

/// The channel may not keep the old view across the page going off and on,
/// so the board published before is sent again once the page is opened.
#[test]
fn a_page_turned_off_and_reopened_gets_its_board_again() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    let first = queue.ready(false);
    queue.finish(&first[0], &PageVerdict::Published);
    queue.want(&work, board(&["b"]));
    let second = queue.ready(false);
    queue.finish(&second[0], &PageVerdict::Off);
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    assert_eq!(sent(&mut queue), [["a"]]);
}

#[test]
fn each_verdict_leaves_the_page_as_it_says() {
    let cases = [
        (
            PageVerdict::Off,
            Settled::Unavailable { first: true },
            Viewer::Unavailable,
            false,
            "a page turned off stops until opened again",
        ),
        (
            PageVerdict::Transient("ratelimited".into()),
            Settled::Retry,
            Viewer::Viewing,
            true,
            "passing trouble waits for the next opening",
        ),
        (
            PageVerdict::Refused("invalid_blocks".into()),
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
            queue.finish(&ready[0], &PageVerdict::Off),
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
        queue.finish(&in_flight[0], &PageVerdict::Published),
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
        queue.finish(&ready[0], &PageVerdict::Off);
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
        queue.finish(publication, &PageVerdict::Transient("ratelimited".into()));
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

/// A tap resolves against the board on screen or the one on its way there,
/// and only under the identity that opened the page. Tapping opens the page
/// and keeps the result for every later board.
#[test]
fn a_tap_resolves_against_the_shown_or_coming_board() {
    use super::super::board::Outcome;
    use super::super::token::{Act, Numbering, Tap};
    let work = scope("work", "bot");
    let mut numbering = Numbering::default();
    let numbered = |names: &[&str], numbering: &mut Numbering| {
        let mut board = board(names);
        board.number(numbering);
        board
    };
    let tap = |board: &Board| Tap {
        generation: board.generation(),
        index: 0,
        act: Act::Select,
    };
    let mut queue = PublishQueue::default();
    let shown = numbered(&["a"], &mut numbering);
    assert_eq!(queue.resolve(&work, tap(&shown)), None, "no page yet");
    queue.tapped(&work, None);
    assert_eq!(queue.viewer(&work), Some(Viewer::Viewing), "a tap opens it");
    assert_eq!(queue.outcome(&work), None);
    queue.want(&work, shown.clone());
    let publication = queue.ready(false).remove(0);
    assert_eq!(
        queue.resolve(&work, tap(&shown)),
        Some(shown.rows()[0].pane),
        "in flight, it may already be on screen"
    );
    let coming = numbered(&["b"], &mut numbering);
    queue.want(&work, coming.clone());
    queue.finish(&publication, &PageVerdict::Published);
    let second = queue.ready(false).remove(0);
    let next = numbered(&["c"], &mut numbering);
    queue.want(&work, next.clone());
    assert_eq!(
        queue.resolve(&work, tap(&coming)),
        Some(coming.rows()[0].pane),
        "in flight after the wanted board moved on"
    );
    queue.finish(&second, &PageVerdict::Published);
    for (board, label) in [(&coming, "shown"), (&next, "coming")] {
        assert_eq!(
            queue.resolve(&work, tap(board)),
            Some(board.rows()[0].pane),
            "{label}"
        );
    }
    assert_eq!(queue.resolve(&scope("work", "other"), tap(&shown)), None);
    let outcome = Outcome {
        at: 1,
        text: "Selected".into(),
    };
    queue.tapped(&work, Some(outcome.clone()));
    queue.tapped(&work, None);
    assert_eq!(queue.outcome(&work), Some(&outcome), "kept until replaced");
}
