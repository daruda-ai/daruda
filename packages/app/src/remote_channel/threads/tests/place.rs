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

    assert_eq!(
        threads.place(&work(), pane, &claude, NOW),
        (None, None),
        "no thread yet"
    );
    threads.remember(&work(), "t1".into(), claude.key.clone(), "1.1".into(), NOW);
    assert_eq!(
        threads.place(&work(), pane, &claude, NOW),
        (Some("1.1".into()), None),
        "the same conversation reuses its thread"
    );

    let cleared = conversation("claude", "s2");
    assert_eq!(
        threads.place(&work(), pane, &cleared, NOW),
        (
            Some("1.1".into()),
            Some(s::remote_channel::thread_new_conversation())
        ),
        "a new conversation of the same agent continues the thread"
    );

    let switched = conversation("codex", "s3");
    assert_eq!(
        threads.place(&work(), pane, &switched, NOW),
        (
            Some("1.1".into()),
            Some(s::remote_channel::thread_switched_agent("codex name"))
        ),
        "another agent continues the thread and is named"
    );

    assert_eq!(
        threads.place(&scope("work", "other", "D1"), pane, &switched, NOW),
        (None, None),
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
    use super::super::roots::RootFace;
    use super::super::status::{Ending, PaneActivity, ShownStatus};
    let mut threads = Threads::load();
    let claude = conversation("claude", "s1");
    // A previous run left this thread; this run knows nothing of its root.
    threads.remember(&work(), "t1".into(), claude.key.clone(), "1.1".into(), NOW);
    let pane = pane(5);
    let ended = PaneActivity::Ended(Ending::Completed);
    threads.note_activity(pane, ended, NOW).unwrap();
    assert_eq!(
        threads.roots_ref().wanted("work", "1.1"),
        None,
        "not posted yet"
    );

    let (root, _) = threads.place(&work(), pane, &claude, NOW);
    threads.want_root_after_place(&work(), pane, root.as_deref(), &claude);
    assert_eq!(
        threads.roots_ref().wanted("work", "1.1"),
        Some(RootFace {
            name: claude.name.clone(),
            shown: ShownStatus::Ended(Ending::Completed),
        })
    );
}

/// Results and questions surface in the conversation; progress stays in the
/// thread, and a post that starts its thread is at the bottom already.
#[test]
fn only_what_the_person_must_see_surfaces_from_a_thread() {
    use super::super::driver::surfaces;
    use super::super::status::{Ending, ShownStatus};
    let ended = Some(ShownStatus::Ended(Ending::Completed));
    let working = Some(ShownStatus::Working { since: NOW });
    let cases = [
        (true, false, ended, true, "a turn's result"),
        (true, true, working, true, "a question with buttons"),
        (true, false, working, false, "progress"),
        (true, false, None, false, "nothing known yet"),
        (false, true, ended, false, "starts its own thread"),
    ];
    for (existing_root, carded, shown, want, label) in cases {
        assert_eq!(surfaces(existing_root, carded, shown), want, "{label}");
    }
}
