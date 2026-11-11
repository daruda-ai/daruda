use super::super::driver::drop_header_under_root;
use super::super::state::Threads;
use super::*;
use crate::remote_channel::bridge::MessageTail;
use crate::remote_channel::transport::Message;
use crate::surface::strings as s;

/// One pane's messages over time, as the outbound queue places them.
#[test]
fn a_pane_keeps_its_thread_across_conversations() {
    let mut threads = Threads::load();
    let pane = pane(7);
    let claude = conversation("claude", "s1");
    let reply = |divider: Option<String>| Placement::Reply {
        root: "1.1".into(),
        divider,
        broadcast: false,
    };
    let placed = |threads: &mut Threads, scope: &Scope, conversation: &PaneConversation| {
        plan(threads, scope, pane, conversation, PingWeight::Progress).placement
    };

    start_thread(&mut threads, &work(), pane, &claude, "1.1");
    assert_eq!(
        placed(&mut threads, &work(), &claude),
        reply(None),
        "the same conversation reuses its thread"
    );
    assert_eq!(
        placed(&mut threads, &work(), &conversation("claude", "s2")),
        reply(Some(s::remote_channel::thread_new_conversation())),
        "a new conversation of the same agent continues the thread"
    );
    let switched = conversation("codex", "s3");
    assert_eq!(
        placed(&mut threads, &work(), &switched),
        reply(Some(s::remote_channel::thread_switched_agent("codex name"))),
        "another agent continues the thread and is named"
    );
    assert_eq!(
        placed(&mut threads, &scope("work", "other", "D1"), &switched),
        Placement::Start,
        "another bot starts its own thread"
    );
}

#[test]
fn a_reply_under_the_root_drops_the_header_unless_it_is_all_there_is() {
    let cases = [
        (MessageTail::Markdown("done".into()), "", "body present"),
        (
            MessageTail::Plain("  ".into()),
            "my_project",
            "blank body keeps the header",
        ),
    ];
    for (tail, header, label) in cases {
        let mut message = Message {
            header: "my_project".into(),
            tail,
            keyboard: None,
        };
        drop_header_under_root(&mut message);
        assert_eq!(message.header, header, "{label}");
    }
}

/// The defect this guards: after a restart this run did not know what a root
/// showed, so a root left "working" stayed so until the status changed
/// again. A post into an existing thread now brings its root up to date.
#[test]
fn a_post_into_an_existing_thread_brings_its_root_up_to_date() {
    use super::super::root_queue::RootFace;
    use super::super::status::{Ending, PaneActivity, ShownStatus};
    let mut threads = Threads::load();
    let claude = conversation("claude", "s1");
    // A previous run left this thread; this run knows nothing of its root.
    threads.remember(&work(), "t1".into(), claude.key.clone(), "1.1".into(), NOW);
    let pane = pane(5);
    let ended = PaneActivity::Ended(Ending::Completed);
    threads.note_activity(pane, ended, NOW).unwrap();
    assert_eq!(threads.wanted("work", "1.1"), None, "not posted yet");

    plan(&mut threads, &work(), pane, &claude, PingWeight::Progress);
    assert_eq!(
        threads.wanted("work", "1.1"),
        Some(RootFace {
            name: claude.name.clone(),
            shown: ShownStatus::Ended(Ending::Completed),
        })
    );
}

/// What its sender marked an outcome surfaces in the conversation; progress
/// stays in the thread, and a post that starts its thread is at the bottom
/// already.
#[test]
fn only_an_outcome_surfaces_from_a_thread() {
    let mut threads = Threads::load();
    let pane = pane(8);
    let claude = conversation("claude", "s1");
    start_thread(&mut threads, &work(), pane, &claude, "1.1");
    for (weight, want, label) in [
        (PingWeight::Outcome, true, "an answer, an end, a question"),
        (PingWeight::Progress, false, "progress"),
    ] {
        let placement = plan(&mut threads, &work(), pane, &claude, weight).placement;
        assert!(
            matches!(placement, Placement::Reply { broadcast, .. } if broadcast == want),
            "{label}"
        );
    }
}

/// The home page links a pane to the thread it last posted in, and only
/// under the identity that made it.
#[test]
fn a_pane_names_the_root_of_its_thread_in_its_scope() {
    let mut threads = Threads::load();
    let speaker = pane(7);
    start_thread(
        &mut threads,
        &work(),
        speaker,
        &conversation("claude", "s1"),
        "1.1",
    );
    let cases = [
        (work(), speaker, Some("1.1"), "the pane's own thread"),
        (scope("work", "other", "D1"), speaker, None, "another bot"),
        (
            scope("home", "bot", "D1"),
            speaker,
            None,
            "another connection",
        ),
        (work(), pane(8), None, "a pane that never posted"),
    ];
    for (scope, pane, want, label) in cases {
        assert_eq!(
            threads.root_of_pane(&scope, pane).as_deref(),
            want,
            "{label}"
        );
    }
}
