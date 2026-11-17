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
