//! A page's thread links: asked once each, kept for the page that asked.

use super::super::publish_queue::PublishQueue;
use super::*;

/// The roots `scope`'s page asks links for now, none without a page.
fn ask(queue: &mut PublishQueue, scope: &Scope, roots: &[&str]) -> Vec<String> {
    queue
        .ask_links(scope, roots.iter().copied())
        .map_or_else(Vec::new, |ask| ask.roots)
}

/// A page asks once for each thread's link: a pending ask is not repeated,
/// a known link stays, and one that failed or was refused waits for the
/// person's next opening. Each failure and refusal is told once per root;
/// another identity's page asks anew, and a link landing for the replaced
/// page is stale.
///
/// The defect this guards: a link that kept failing (a reply that never
/// names its trouble) was asked again, and the page built anew, on every
/// retry tick; and a refusal fixable without a new token (the bot added to
/// the channel) was kept for as long as the page.
#[test]
fn a_page_asks_each_thread_link_once_until_the_next_opening() {
    use super::super::publish_queue::Learned;
    use crate::remote_channel::transport::LinkFailure;
    let failed = Err(LinkFailure::Transient("unknown_error".into()));
    let refused = Err(LinkFailure::Refused("not_in_channel".into()));
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
        queue.learn_link(&work, epoch, "3.3", &refused),
        Some(Learned::Refused { first: true })
    );
    assert_eq!(linked(&queue, &work), None, "pending: no link yet");
    assert_eq!(
        queue.learn_link(&work, epoch, "1.1", &Ok("https://x/p1".into())),
        Some(Learned::Known)
    );
    assert_eq!(linked(&queue, &work).as_deref(), Some("https://x/p1"));
    assert_eq!(
        queue.learn_link(&work, epoch, "2.2", &failed),
        Some(Learned::Failed { first: true })
    );
    for tick in 0..3 {
        assert!(!queue.retry(|_| true), "tick {tick}: nothing to build");
    }
    assert!(
        ask(&mut queue, &work, &["1.1", "2.2", "3.3"]).is_empty(),
        "the retry tick asks neither again"
    );
    queue.opened(&work);
    assert_eq!(
        ask(&mut queue, &work, &["1.1", "2.2", "3.3"]),
        ["2.2", "3.3"],
        "the next opening asks both again, not the known one"
    );
    assert_eq!(
        queue.learn_link(&work, epoch, "2.2", &failed),
        Some(Learned::Failed { first: false }),
        "a failure told once"
    );
    assert_eq!(
        queue.learn_link(&work, epoch, "3.3", &refused),
        Some(Learned::Refused { first: false }),
        "a refusal told once"
    );
    let other = scope("work", "other");
    queue.opened(&other);
    assert_eq!(
        queue.learn_link(&work, epoch, "2.2", &Ok("https://x/p2".into())),
        None,
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
        None,
        "the replaced page's answer"
    );
    for root in ["1.1", "2.2"] {
        assert_eq!(
            queue.learn_link(&work, new.epoch, root, &link(1)),
            Some(Learned::Known),
            "{root}: the new page's own answer"
        );
    }
    assert!(
        ask(&mut queue, &work, &["1.1", "2.2"]).is_empty(),
        "nothing pending is left unanswered"
    );
}
