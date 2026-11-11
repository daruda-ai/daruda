use super::super::publish_queue::{Publication, PublishQueue, Settled, Viewer};
use super::*;
use crate::remote_channel::transport::PageVerdict;

fn sent(queue: &mut PublishQueue) -> Vec<Vec<String>> {
    queue
        .ready()
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
        queue.finish(&first[0], &PageVerdict::Published),
        Settled::Published
    );
    let second = queue.ready();
    assert_eq!(second[0].board, board(&["c"]), "what changed meanwhile");
    queue.finish(&second[0], &PageVerdict::Published);
    queue.want(&work, board(&["c"]));
    assert!(queue.ready().is_empty(), "an identical board is skipped");
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
        let first = queue.ready();
        queue.finish(&first[0], &PageVerdict::Published);
        queue.want(&work, board(&["a"]));
        let in_flight = queue.ready();
        assert_eq!(in_flight[0].board, board(&["a"]), "{label}");
        queue.want(&work, board(&[later]));
        assert!(queue.ready().is_empty(), "{label}: one at a time");
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
    let first = queue.ready();
    queue.finish(&first[0], &PageVerdict::Published);
    queue.want(&work, board(&["b"]));
    let second = queue.ready();
    queue.finish(&second[0], &PageVerdict::Off);
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    assert_eq!(sent(&mut queue), [["a"]]);
}

/// Whatever the verdict, nothing goes out by itself afterwards: the next
/// opening builds the page anew, and a board wanted then is sent.
#[test]
fn each_verdict_leaves_the_page_as_it_says() {
    let cases = [
        (
            PageVerdict::Off,
            Settled::Unavailable { first: true },
            Viewer::Unavailable,
            "a page turned off stops until opened again",
        ),
        (
            PageVerdict::Transient("ratelimited".into()),
            Settled::Retry,
            Viewer::Viewing,
            "passing trouble waits for the next opening",
        ),
        (
            PageVerdict::Refused("invalid_blocks".into()),
            Settled::Dropped,
            Viewer::Viewing,
            "a refused board is dropped",
        ),
    ];
    for (verdict, settled, viewer, label) in cases {
        let work = scope("work", "bot");
        let mut queue = PublishQueue::default();
        queue.opened(&work);
        queue.want(&work, board(&["a"]));
        let ready = queue.ready();
        assert_eq!(queue.finish(&ready[0], &verdict), settled, "{label}");
        assert_eq!(queue.viewer(&work), Some(viewer), "{label}");
        assert!(queue.ready().is_empty(), "{label}: nothing sent unasked");
        queue.opened(&work);
        assert!(
            queue.ready().is_empty(),
            "{label}: the next opening sends nothing by itself"
        );
        // The panes moved on meanwhile; the same board again is another
        // case for a refused one.
        queue.want(&work, board(&["b"]));
        assert_eq!(sent(&mut queue), [["b"]], "{label}: the page built then");
    }
}

/// The defect this guards: a board wanted while a refused one was on its
/// way waited for the next change. It differs from the refused board, so it
/// goes at once; the refused board itself is not sent again.
#[test]
fn a_board_wanted_during_a_refused_publish_goes_at_once() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    let refused = queue.ready();
    queue.want(&work, board(&["b"]));
    assert_eq!(
        queue.finish(&refused[0], &PageVerdict::Refused("invalid_blocks".into())),
        Settled::Dropped
    );
    assert_eq!(sent(&mut queue), [["b"]], "the newer board, not on a retry");
}

/// The defect this guards: an unrelated rebuild wanted the refused board
/// again, and it was sent again, to be refused again. It stays unsent until
/// another board has gone out.
#[test]
fn a_refused_board_is_not_sent_again_until_another_goes_out() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    let refused = queue.ready();
    queue.finish(&refused[0], &PageVerdict::Refused("invalid_blocks".into()));
    queue.want(&work, board(&["a"]));
    assert!(queue.ready().is_empty(), "the same board is not sent again");
    queue.want(&work, board(&["b"]));
    let other = queue.ready();
    assert_eq!(other[0].board, board(&["b"]), "another board goes");
    queue.finish(&other[0], &PageVerdict::Published);
    queue.want(&work, board(&["a"]));
    assert_eq!(
        sent(&mut queue),
        [["a"]],
        "once another went out, it may be sent again"
    );
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
    let in_flight = queue.ready();
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

/// A board its connection could not send waits; the next opening builds the
/// page anew rather than sending the board as it was.
#[test]
fn a_held_board_gives_way_to_the_next_opening() {
    let work = scope("work", "bot");
    let mut queue = PublishQueue::default();
    queue.opened(&work);
    queue.want(&work, board(&["a"]));
    let ready = queue.ready();
    queue.hold(&ready[0]);
    assert!(queue.ready().is_empty());
    queue.opened(&work);
    assert!(queue.ready().is_empty(), "not the held board");
    queue.want(&work, board(&["b"]));
    assert_eq!(sent(&mut queue), [["b"]]);
}

/// Only a page the person opened is kept up to date as panes change: one
/// never opened, or turned off, gets nothing unasked.
#[test]
fn only_a_viewed_page_takes_a_new_board() {
    let opened = |queue: &mut PublishQueue| queue.opened(&scope("work", "bot"));
    let turned_off = |queue: &mut PublishQueue| {
        queue.opened(&scope("work", "bot"));
        queue.want(&scope("work", "bot"), board(&["a"]));
        let ready = queue.ready();
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

/// A page whose board failed for a passing reason, or whose connection could
/// not send, is due a new board on the retry tick, not on an ordinary flush;
/// the board that waited was built while things stood otherwise, so it is
/// not sent as it was.
#[test]
fn a_waiting_page_is_built_anew_on_the_retry_tick() {
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
        let ready = queue.ready();
        wait(&mut queue, &ready[0]);
        assert!(queue.ready().is_empty(), "{label}: not on a flush");
        assert!(queue.retry(), "{label}: due on the retry tick");
        assert!(queue.ready().is_empty(), "{label}: not the old board");
        assert!(!queue.retry(), "{label}: nothing waits any more");
        queue.want(&work, board(&["b"]));
        assert_eq!(sent(&mut queue), [["b"]], "{label}: the page built anew");
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
    queue.retry();
    assert!(queue.ready().is_empty(), "nothing sent for the gone page");
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
    let publication = queue.ready().remove(0);
    assert_eq!(
        queue.resolve(&work, tap(&shown)),
        Some(shown.rows()[0].pane),
        "in flight, it may already be on screen"
    );
    let coming = numbered(&["b"], &mut numbering);
    queue.want(&work, coming.clone());
    queue.finish(&publication, &PageVerdict::Published);
    let second = queue.ready().remove(0);
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

/// The roots `scope`'s page asks links for now, none without a page.
fn ask(queue: &mut PublishQueue, scope: &Scope, roots: &[&str]) -> Vec<String> {
    queue
        .ask_links(scope, roots.iter().copied())
        .map_or_else(Vec::new, |ask| ask.roots)
}

/// A page asks once for each thread's link: a pending ask is not repeated,
/// a known link stays, a failed one waits for the retry tick or the next
/// opening, and a refused one is not asked again. A failure is told once per
/// root; another identity's page asks anew, and a link landing for the
/// replaced page is stale.
#[test]
fn a_page_asks_each_thread_link_once_until_a_failure_is_retried() {
    use super::super::publish_queue::Learned;
    use crate::remote_channel::transport::LinkFailure;
    let failed = Err(LinkFailure::Transient("ratelimited".into()));
    let work = scope("work", "bot");
    let linked = |queue: &PublishQueue, scope| {
        let mut threaded = row("a", None);
        threaded.thread = Some("1.1".into());
        let mut board = Board::new(vec![threaded], 10, None);
        queue.link(scope, &mut board);
        board.rows()[0].link.clone()
    };
    let mut queue = PublishQueue::default();
    assert!(queue.ask_links(&work, ["1.1"]).is_none(), "no page, no ask");
    queue.opened(&work);
    let first = queue.ask_links(&work, ["1.1", "2.2", "3.3"]).unwrap();
    assert_eq!(first.roots, ["1.1", "2.2", "3.3"]);
    let epoch = first.epoch;
    assert!(
        ask(&mut queue, &work, &["1.1", "2.2", "3.3"]).is_empty(),
        "pending asks are not repeated"
    );
    assert_eq!(
        queue.learn_link(
            &work,
            epoch,
            "3.3",
            &Err(LinkFailure::Refused("message_not_found".into()))
        ),
        Learned::Refused
    );
    assert_eq!(linked(&queue, &work), None, "pending: no link yet");
    assert_eq!(
        queue.learn_link(&work, epoch, "1.1", &Ok("https://x/p1".into())),
        Learned::Known
    );
    assert_eq!(linked(&queue, &work).as_deref(), Some("https://x/p1"));
    assert_eq!(
        queue.learn_link(&work, epoch, "2.2", &failed),
        Learned::Failed { first: true }
    );
    assert!(
        ask(&mut queue, &work, &["1.1", "2.2"]).is_empty(),
        "a failed link waits for a later trigger"
    );
    assert!(queue.retry(), "the retry tick asks again");
    assert_eq!(
        ask(&mut queue, &work, &["1.1", "2.2", "3.3"]),
        ["2.2"],
        "but not for a refused link"
    );
    assert_eq!(
        queue.learn_link(&work, epoch, "2.2", &failed),
        Learned::Failed { first: false },
        "told once"
    );
    queue.opened(&work);
    assert_eq!(
        ask(&mut queue, &work, &["2.2", "3.3"]),
        ["2.2"],
        "the next opening asks again, but not for a refused link"
    );
    let other = scope("work", "other");
    queue.opened(&other);
    assert_eq!(
        queue.learn_link(&work, epoch, "2.2", &Ok("https://x/p2".into())),
        Learned::Stale,
        "the old identity's page is gone"
    );
    assert_eq!(linked(&queue, &other), None, "links do not carry over");
    assert_eq!(ask(&mut queue, &other, &["1.1"]), ["1.1"]);
}

/// The defect this guards: the identity flipped away and back while a page's
/// links were being asked, the replaced page's answer settled the new page's
/// ask, and the new page's own answer, found settled, ended its asking with
/// the rest still pending. Answers count only for the page that asked.
#[test]
fn a_page_made_again_under_the_same_identity_settles_only_its_own_asks() {
    use super::super::publish_queue::Learned;
    let work = scope("work", "bot");
    let link = |n: u8| Ok(format!("https://x/p{n}"));
    let mut queue = PublishQueue::default();
    queue.opened(&work);
    let old = queue.ask_links(&work, ["1.1", "2.2"]).unwrap().epoch;
    queue.opened(&scope("work", "other"));
    queue.opened(&work);
    let new = queue.ask_links(&work, ["1.1", "2.2"]).unwrap();
    assert_eq!(new.roots, ["1.1", "2.2"], "the new page asks for itself");
    assert_eq!(
        queue.learn_link(&work, old, "1.1", &link(1)),
        Learned::Stale,
        "the replaced page's answer"
    );
    for root in ["1.1", "2.2"] {
        assert_eq!(
            queue.learn_link(&work, new.epoch, root, &link(1)),
            Learned::Known,
            "{root}: the new page's own answer"
        );
    }
    assert!(
        ask(&mut queue, &work, &["1.1", "2.2"]).is_empty(),
        "nothing pending is left unanswered"
    );
}
