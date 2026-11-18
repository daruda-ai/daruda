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

#[test]
fn a_read_sent_before_a_resolve_landed_keeps_its_mark() {
    let mut tickets = Tickets::default();
    let mut work = ThreadsWork::default();
    let before = tickets.issue();
    work.begin("a", ThreadAction::Resolving);
    work.finish("a", ThreadAction::Resolving, None, tickets.issue());
    work.threads_read(before, None, |_| String::new());
    assert!(
        work.resolved_here("a"),
        "that read may still list it as open"
    );
    let after = tickets.issue();
    work.threads_read(after, None, |_| String::new());
    assert!(!work.resolved_here("a"), "a later read no longer lists it");
}

/// A failed reply checked against the threads read after it: early answers
/// leave it checking, then it failed after all unless it shows as posted.
#[test]
fn a_failed_reply_is_checked_only_by_a_read_sent_after_it() {
    let mut tickets = Tickets::default();
    let (before, after, later) = (tickets.issue(), tickets.issue(), tickets.issue());
    let checking = || ThreadWork {
        op: Some(ThreadOp::Checking {
            reason: "timed out".into(),
            after,
        }),
        ..ThreadWork::default()
    };
    // (label, answered at, posted, op after, found posted)
    let cases = [
        (
            "an earlier read says nothing",
            before,
            false,
            checking().op,
            false,
        ),
        (
            "not posted: failed, with its reason",
            later,
            false,
            Some(ThreadOp::Failed(ThreadAction::Replying, "timed out".into())),
            false,
        ),
        ("posted after all", after, true, None, true),
    ];
    for (label, ticket, posted, op, found) in cases {
        let mut work = checking();
        assert_eq!(work.checked(ticket, |_| posted), found, "{label}");
        assert_eq!(work.op, op, "{label}");
    }
    let mut threads = ThreadsWork::default();
    threads.work.insert("t".into(), checking());
    assert!(
        !threads.begin("t", ThreadAction::Replying),
        "nothing is posted again while it is checked"
    );
}
