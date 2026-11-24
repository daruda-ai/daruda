use super::*;
use crate::workspace::pages::pull_requests::detail::read_slot::Tickets;

#[test]
fn each_thread_runs_one_action_and_keeps_its_own_outcome() {
    let mut tickets = Tickets::default();
    let mut work = ThreadsWork::default();
    assert!(work.begin("a", ThreadAction::Resolving));
    assert!(
        !work.begin("a", ThreadAction::Replying),
        "one at a time per thread"
    );
    assert!(
        work.begin("b", ThreadAction::Replying),
        "another thread is free"
    );

    assert!(work.finish(
        "b",
        ThreadAction::Replying,
        Some("HTTP 502".into()),
        tickets.issue()
    ));
    assert!(work.finish("a", ThreadAction::Resolving, None, tickets.issue()));
    assert_eq!(
        work.get("b").and_then(|w| w.op.clone()),
        Some(ThreadOp::Failed(ThreadAction::Replying, "HTTP 502".into())),
        "a's success does not clear b's failure"
    );
    assert!(work.resolved_here("a"));
    assert!(
        !work.finish("a", ThreadAction::Resolving, None, tickets.issue()),
        "an action lands once"
    );

    assert!(work.begin("a", ThreadAction::Unresolving));
    assert!(work.finish("a", ThreadAction::Unresolving, None, tickets.issue()));
    assert!(!work.resolved_here("a"));
}

/// "Done." sent by `me` after comment `c1`, failed with "timed out".
fn sent() -> SentReply {
    SentReply {
        body: "Done.".into(),
        after: Some("c1".into()),
        by: Some("me".into()),
        reason: "timed out".into(),
        cut_off: false,
    }
}

/// `sent()` being checked by reads under `after` or later.
fn pending(after: ReadTicket) -> PendingReply {
    PendingReply {
        after,
        sent: sent(),
    }
}

/// Thread `t` read with these comments, each `(id, body)`, all by `me`.
fn thread_with(comments: &[(&str, &str)]) -> ReviewThread {
    ReviewThread {
        id: "t".into(),
        comments: comments
            .iter()
            .map(|(id, body)| ReviewComment {
                id: (*id).into(),
                url: None,
                author: Some("me".into()),
                body: (*body).into(),
            })
            .collect(),
        ..Default::default()
    }
}

fn with(op: ThreadOp) -> ThreadWork {
    ThreadWork {
        op: Some(op),
        ..ThreadWork::default()
    }
}

#[test]
fn a_read_sent_before_a_resolve_landed_keeps_its_mark() {
    let mut tickets = Tickets::default();
    let mut work = ThreadsWork::default();
    let read = ReviewThreads::default();
    let before = tickets.issue();
    work.begin("a", ThreadAction::Resolving);
    work.finish("a", ThreadAction::Resolving, None, tickets.issue());
    work.threads_read(before, Some(&read));
    assert!(
        work.resolved_here("a"),
        "that read may still list it as open"
    );
    let failed = tickets.issue();
    work.threads_read(failed, None);
    assert!(
        work.resolved_here("a"),
        "a failed read leaves the older threads shown"
    );
    let after = tickets.issue();
    work.threads_read(after, Some(&read));
    assert!(!work.resolved_here("a"), "a later read no longer lists it");
}

/// A failed read cannot say whether a failed reply posted: Post keeps
/// waiting until a read that arrives says.
#[test]
fn a_failed_reply_stays_unconfirmed_while_its_check_cannot_be_read() {
    let mut tickets = Tickets::default();
    let (before, after) = (tickets.issue(), tickets.issue());
    let mut threads = ThreadsWork::default();
    threads
        .work
        .insert("t".into(), with(ThreadOp::Checking(pending(after))));
    let op = |threads: &ThreadsWork| threads.get("t").and_then(|w| w.op.clone());

    assert!(threads.threads_read(before, None).is_empty());
    assert_eq!(
        op(&threads),
        Some(ThreadOp::Checking(pending(after))),
        "an earlier read says nothing"
    );
    assert!(threads.threads_read(tickets.issue(), None).is_empty());
    assert_eq!(op(&threads), Some(ThreadOp::Unconfirmed(pending(after))));
    assert!(
        !threads.begin("t", ThreadAction::Replying),
        "nothing is posted again while unconfirmed"
    );
    let listed = ReviewThreads {
        threads: vec![thread_with(&[("c1", "Rename this.")])],
        truncated: false,
    };
    assert_eq!(
        threads.threads_read(tickets.issue(), Some(&listed)),
        vec![("t".to_owned(), Settled::NotPosted)],
        "a read that arrived settles it"
    );
    assert_eq!(
        op(&threads),
        Some(ThreadOp::Failed(ThreadAction::Replying, "timed out".into()))
    );
}

/// A failed reply checked against the threads read after it: early answers
/// leave it checking; a read that cannot tell leaves it unconfirmed; else it
/// failed after all unless it shows as posted.
#[test]
fn a_failed_reply_is_checked_only_by_a_read_sent_after_it() {
    let mut tickets = Tickets::default();
    let (before, after, later) = (tickets.issue(), tickets.issue(), tickets.issue());
    let checking = || with(ThreadOp::Checking(pending(after)));
    let failed = Some(ThreadOp::Failed(ThreadAction::Replying, "timed out".into()));
    // (label, answered at, what the read says, op after, settled)
    let cases = [
        (
            "an earlier read says nothing",
            before,
            Some(true),
            checking().op,
            None,
        ),
        (
            "not posted: failed, with its reason",
            later,
            Some(false),
            failed,
            Some(Settled::NotPosted),
        ),
        (
            "cannot tell: unconfirmed",
            later,
            None,
            Some(ThreadOp::Unconfirmed(pending(after))),
            None,
        ),
        (
            "posted after all",
            after,
            Some(true),
            None,
            Some(Settled::Posted),
        ),
    ];
    for (label, ticket, says, op, settled) in cases {
        let mut work = checking();
        assert_eq!(work.checked(ticket, |_| says), settled, "{label}");
        assert_eq!(work.op, op, "{label}");
    }
    let mut threads = ThreadsWork::default();
    threads.work.insert("t".into(), checking());
    assert!(
        !threads.begin("t", ThreadAction::Replying),
        "nothing is posted again while it is checked"
    );
}

/// Only a comment after the one the reply followed, with its text and by
/// its account, can be it; a read that cannot see that far cannot tell.
#[test]
fn a_sent_reply_is_looked_for_after_the_comment_it_followed() {
    let after = |more: &[(&str, &str)]| {
        let mut all = vec![("c1", "Rename this.")];
        all.extend_from_slice(more);
        thread_with(&all)
    };
    let mut cut_short = after(&[("c2", "Done.")]);
    cut_short.more_comments = true;
    let mut by_someone = after(&[("c2", "Done.")]);
    by_someone.comments[1].author = Some("someone".into());
    // (label, sent, thread as read, arrived)
    let cases = [
        (
            "the same text before it is not it",
            sent(),
            Some(thread_with(&[("c0", "Done."), ("c1", "Rename this.")])),
            Some(false),
        ),
        (
            "after it, even with another reply after that",
            sent(),
            Some(after(&[("c2", "Done."), ("c3", "Thanks.")])),
            Some(true),
        ),
        (
            "other line endings and trailing spaces are the same text",
            SentReply {
                body: "Done.\nAll of it.".into(),
                ..sent()
            },
            Some(after(&[("c2", "Done.  \r\nAll of it.\r\n")])),
            Some(true),
        ),
        (
            "the same text by someone else is not it",
            sent(),
            Some(by_someone.clone()),
            Some(false),
        ),
        (
            "the account not known: any author's comment counts",
            SentReply { by: None, ..sent() },
            Some(by_someone),
            Some(true),
        ),
        (
            "cut off: missing may post yet",
            SentReply {
                cut_off: true,
                ..sent()
            },
            Some(after(&[])),
            None,
        ),
        (
            "cut off, found: posted",
            SentReply {
                cut_off: true,
                ..sent()
            },
            Some(after(&[("c2", "Done.")])),
            Some(true),
        ),
        ("thread not listed (resolved since)", sent(), None, None),
        ("its end not read", sent(), Some(cut_short), None),
        (
            "the comment it followed is gone",
            sent(),
            Some(thread_with(&[("c9", "Done.")])),
            None,
        ),
        (
            "sent where the thread's end was not read",
            SentReply {
                after: None,
                ..sent()
            },
            Some(after(&[("c2", "Done.")])),
            None,
        ),
    ];
    for (label, sent, thread, expected) in cases {
        assert_eq!(arrived(&sent, thread.as_ref()), expected, "{label}");
    }
}

/// Sent after the last comment shown, unless the thread's end was not read;
/// only a request cut off waiting, here or by GitHub, may post yet.
#[test]
fn a_reply_goes_after_the_last_comment_shown() {
    let thread = thread_with(&[("c0", "Rename this."), ("c1", "Done?")]);
    let me = || Some("me".into());
    let exit = |stderr: &str| GhError::Exit {
        code: Some(1),
        stderr: stderr.into(),
    };
    // (label, error, cut off)
    let cases = [
        (
            "our own timeout",
            GhError::Timeout(std::time::Duration::from_secs(30)),
            true,
        ),
        ("GitHub's 502, no JSON body", exit("gh: HTTP 502"), true),
        (
            "GitHub's 504 with its message",
            exit("gh: We couldn't respond to your request in time (HTTP 504)"),
            true,
        ),
        (
            "its message alone",
            exit("gh: We couldn't respond to your request in time"),
            true,
        ),
        (
            "refused: it did not post",
            exit("gh: Validation Failed (HTTP 422)"),
            false,
        ),
        ("not found", exit("gh: Not Found (HTTP 404)"), false),
    ];
    for (label, error, cut_off) in cases {
        let failed = Outgoing::to(Some(&thread), "Done.".into(), me()).failed(&error);
        assert_eq!(
            (
                failed.after.as_deref(),
                failed.by.as_deref(),
                failed.cut_off
            ),
            (Some("c1"), Some("me"), cut_off),
            "{label}"
        );
    }
    let mut cut_short = thread.clone();
    cut_short.more_comments = true;
    assert_eq!(
        Outgoing::to(Some(&cut_short), "Done.".into(), me()).after,
        None
    );
    assert_eq!(Outgoing::to(None, "Done.".into(), me()).after, None);
}

/// Only a reply that could not be confirmed can be said not posted; it then
/// failed with its reason and may go again.
#[test]
fn a_person_can_say_an_unconfirmed_reply_did_not_post() {
    let mut tickets = Tickets::default();
    let after = tickets.issue();
    let mut work = with(ThreadOp::Checking(pending(after)));
    assert!(!work.not_posted(), "still being checked");
    assert_eq!(work.op, Some(ThreadOp::Checking(pending(after))));

    let mut threads = ThreadsWork::default();
    threads
        .work
        .insert("t".into(), with(ThreadOp::Unconfirmed(pending(after))));
    assert!(threads.work.get_mut("t").unwrap().not_posted());
    assert_eq!(
        threads.get("t").and_then(|w| w.op.clone()),
        Some(ThreadOp::Failed(ThreadAction::Replying, "timed out".into()))
    );
    assert!(
        threads.begin("t", ThreadAction::Replying),
        "it may go again"
    );
}

#[test]
fn a_posted_reply_appears_once_across_an_overlapping_read() {
    use super::super::ReadSlot;
    use crate::workspace::tests::pull_request_detail::item;

    let posted = thread_with(&[("posted", "Done")]).comments.remove(0);
    for already_present in [false, true] {
        let mut state = PrDetailState::new(PrDetailId(1), item(7, "feat"));
        let mut thread = thread_with(&[("first", "Fix")]);
        if already_present {
            thread.comments.push(posted.clone());
        }
        state.threads = Some(ReadSlot::settled(Load::Ready(ReviewThreads {
            threads: vec![thread],
            truncated: false,
        })));
        let (old, _) = state.reread_threads();
        let Some(TabRead::Threads(next)) = state.record_posted_reply("t", &posted) else {
            panic!("a read that began before posting is superseded");
        };
        assert!(next > old);
        let slot = state.threads.as_mut().unwrap();
        assert!(!slot.land(old, Ok(ReviewThreads::default())));
        let Load::Ready(threads) = slot.load() else {
            panic!("shown threads")
        };
        assert_eq!(
            threads.threads[0].comments.len(),
            2,
            "present: {already_present}"
        );
    }
}
