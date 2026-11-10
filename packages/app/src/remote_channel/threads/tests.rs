use super::book::{MAX_AGE_SECS, MAX_SESSIONS, MAX_THREADS, TOUCH_INTERVAL_SECS};
use super::driver::{Threads, drop_header_under_root};
use super::*;
use crate::remote_channel::bridge::{MessageTail, PaneRef};
use crate::remote_channel::transport::Message;
use crate::surface::strings as s;

const NOW: i64 = 1_800_000_000;

fn scope(connection: &str, bot: &str, conversation: &str) -> Scope {
    Scope {
        connection: connection.into(),
        bot: bot.into(),
        conversation: conversation.into(),
    }
}

fn work() -> Scope {
    scope("work", "bot", "D1")
}

fn key(agent: &str, session: &str) -> ConversationKey {
    ConversationKey {
        agent: agent.into(),
        session: session.into(),
    }
}

fn pane(id: u64) -> PaneRef {
    PaneRef {
        workspace: daruda_store::project::WorkspaceUuid::new(),
        pane: id,
    }
}

fn conversation(agent: &str, session: &str) -> PaneConversation {
    PaneConversation {
        key: key(agent, session),
        agent_name: format!("{agent} name"),
        title: "my_project\nClaude".into(),
    }
}

#[test]
fn a_thread_is_found_only_under_the_identity_that_made_it() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    let found = |scope: &Scope| {
        book.for_conversation(scope, &key("claude", "s1"))
            .map(|r| r.root.clone())
    };
    let cases = [
        (work(), Some("1.1".to_owned()), "same identity"),
        (scope("home", "bot", "D1"), None, "another connection"),
        (scope("work", "other", "D1"), None, "another bot"),
        (scope("work", "bot", "D2"), None, "another chat"),
    ];
    for (scope, want, label) in cases {
        assert_eq!(found(&scope), want, "{label}");
    }
    assert!(
        book.for_conversation(&work(), &key("codex", "s1"))
            .is_none(),
        "another agent"
    );
}

#[test]
fn a_conversation_belongs_to_one_thread() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    book.insert(&work(), "t".into(), key("claude", "s1"), "2.2".into(), NOW);
    assert_eq!(book.records().len(), 1, "the older claim is released");
    assert_eq!(book.records()[0].root, "2.2");
}

#[test]
fn a_new_conversation_continues_the_previous_thread() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    // Another thread already holds the conversation being continued into.
    book.insert(&work(), "t".into(), key("claude", "s2"), "9.9".into(), NOW);

    let root = book.continue_with(&work(), &key("claude", "s1"), key("claude", "s2"), NOW + 5);

    assert_eq!(root.as_deref(), Some("1.1"));
    assert_eq!(book.records().len(), 1, "the emptied thread is dropped");
    assert_eq!(
        book.records()[0].sessions,
        vec![key("claude", "s1"), key("claude", "s2")]
    );
    assert_eq!(book.records()[0].last_used, NOW + 5);
    assert_eq!(
        book.continue_with(&work(), &key("claude", "unknown"), key("claude", "s3"), NOW),
        None,
        "nothing to continue"
    );
    assert_eq!(
        book.continue_with(&work(), &key("claude", "s2"), key("claude", "s2"), NOW),
        None,
        "same conversation"
    );
}

#[test]
fn a_thread_keeps_only_its_recent_conversations() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "0"), "1.1".into(), NOW);
    for n in 1..MAX_SESSIONS + 5 {
        book.continue_with(
            &work(),
            &key("claude", &(n - 1).to_string()),
            key("claude", &n.to_string()),
            NOW,
        );
    }
    let sessions = &book.records()[0].sessions;
    assert_eq!(sessions.len(), MAX_SESSIONS);
    assert_eq!(
        sessions.last(),
        Some(&key("claude", &(MAX_SESSIONS + 4).to_string()))
    );
}

#[test]
fn use_is_saved_at_most_once_per_interval() {
    let mut book = Book::default();
    book.insert(&work(), "t".into(), key("claude", "s1"), "1.1".into(), NOW);
    assert!(!book.touch(&work(), "1.1", NOW + TOUCH_INTERVAL_SECS - 1));
    assert!(book.touch(&work(), "1.1", NOW + TOUCH_INTERVAL_SECS));
    assert!(!book.touch(&work(), "unknown", NOW + 10 * TOUCH_INTERVAL_SECS));
}

#[test]
fn pruning_drops_stale_threads_then_keeps_the_newest() {
    let mut book = Book::default();
    book.insert(
        &work(),
        "t".into(),
        key("claude", "old"),
        "0.0".into(),
        NOW - MAX_AGE_SECS - 1,
    );
    for n in 0..MAX_THREADS + 3 {
        book.insert(
            &work(),
            "t".into(),
            key("claude", &n.to_string()),
            n.to_string(),
            NOW - n as i64,
        );
    }
    assert!(book.prune(NOW));
    assert_eq!(book.records().len(), MAX_THREADS);
    assert!(
        book.records().iter().all(|r| r.root != "0.0"),
        "past the age limit"
    );
    assert!(
        book.records()
            .iter()
            .all(|r| r.last_used > NOW - MAX_THREADS as i64),
        "only the newest survive"
    );
    assert!(!book.prune(NOW), "nothing left to drop");
}

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

mod status {
    use super::super::roots::{Finished, RootFace, Roots};
    use super::super::status::{Ending, PaneActivity, ShownStatus};
    use super::*;
    use crate::remote_channel::transport::slack::threads::RootFailure;

    #[test]
    fn work_keeps_the_time_it_began_until_the_turn_ends() {
        let working = |since| ShownStatus::Working { since };
        let needs = |since| ShownStatus::NeedsYou { since };
        let cases = [
            (None, PaneActivity::Working, working(NOW), "work begins now"),
            (
                Some(working(1)),
                PaneActivity::Working,
                working(1),
                "still working",
            ),
            (
                Some(working(1)),
                PaneActivity::AwaitingYou,
                needs(1),
                "paused on the person",
            ),
            (
                Some(needs(1)),
                PaneActivity::Working,
                working(1),
                "resumed after the answer",
            ),
            (
                Some(ShownStatus::Ended(Ending::Completed)),
                PaneActivity::Working,
                working(NOW),
                "a new turn begins anew",
            ),
            (
                Some(working(1)),
                PaneActivity::Ended(Ending::OverageStopped),
                ShownStatus::Ended(Ending::OverageStopped),
                "ended",
            ),
            (
                Some(working(1)),
                PaneActivity::Closed,
                ShownStatus::Closed,
                "closed",
            ),
        ];
        for (previous, activity, want, label) in cases {
            assert_eq!(ShownStatus::next(previous, activity, NOW), want, "{label}");
        }
    }

    /// Emoji names render as Slack's art; U+23F9 with no variation selector
    /// renders as a plain glyph, which is why stopping shows `octagonal_sign`.
    #[test]
    fn every_status_names_a_slack_emoji() {
        let cases = [
            (
                ShownStatus::Working { since: NOW },
                "hourglass_flowing_sand",
            ),
            (ShownStatus::NeedsYou { since: NOW }, "bell"),
            (ShownStatus::Ended(Ending::Completed), "white_check_mark"),
            (ShownStatus::Ended(Ending::Failed), "x"),
            (ShownStatus::Ended(Ending::Stopped), "octagonal_sign"),
            (ShownStatus::Ended(Ending::OverageStopped), "credit_card"),
            (ShownStatus::Closed, "door"),
        ];
        for (shown, emoji) in cases {
            let (name, label) = shown.view();
            assert_eq!(name, emoji, "{shown:?}");
            assert!(!label.is_empty(), "{shown:?}");
        }
    }

    fn face(title: &str, shown: ShownStatus) -> RootFace {
        RootFace {
            title: title.into(),
            shown,
        }
    }

    /// One edit per root at a time, carrying the newest face.
    #[test]
    fn a_root_is_edited_once_at_a_time_with_its_latest_face() {
        let mut roots = Roots::default();
        let working = ShownStatus::Working { since: NOW };
        let done = ShownStatus::Ended(Ending::Completed);
        roots.want(&work(), "t", "1.1".into(), face("title", working));
        let first = roots.ready(false);
        assert_eq!(first.len(), 1);
        assert!(roots.ready(false).is_empty(), "in flight");

        roots.want(&work(), "t", "1.1".into(), face("title", done));
        assert_eq!(roots.finish(&first[0], &Ok(())), Finished::Shown);
        let second = roots.ready(false);
        assert_eq!(second.len(), 1);
        assert_eq!(
            second[0].face.shown, done,
            "the status that changed meanwhile"
        );
    }

    /// The defect this guards: only the status was compared, so a new title
    /// at an unchanged status never reached the root.
    #[test]
    fn a_new_title_alone_updates_the_root() {
        let mut roots = Roots::default();
        let done = ShownStatus::Ended(Ending::Completed);
        roots.shown(&work(), "1.1", face("my_project\nClaude", done));
        roots.want(&work(), "t", "1.1".into(), face("my_project\nClaude", done));
        assert!(roots.ready(false).is_empty(), "already shown");
        roots.want(&work(), "t", "1.1".into(), face("my_project\nCodex", done));
        assert_eq!(roots.ready(false).len(), 1);
    }

    /// The defect this guards: a failed edit was resent on every flush, so a
    /// rate-limited channel was pressed by each other root's change.
    #[test]
    fn a_passing_failure_waits_for_the_retry_tick() {
        let mut roots = Roots::default();
        let done = ShownStatus::Ended(Ending::Completed);
        roots.want(&work(), "t", "1.1".into(), face("title", done));
        let sent = roots.ready(false);
        let failed = Err(RootFailure::Transient("ratelimited".into()));
        assert_eq!(roots.finish(&sent[0], &failed), Finished::Retry);
        assert!(roots.ready(false).is_empty(), "not on an ordinary flush");
        assert_eq!(roots.ready(true).len(), 1, "on the retry tick");
    }

    /// Only a missing root lets its wanted face go as gone; a refused edit
    /// drops the face but the caller keeps the thread.
    #[test]
    fn a_missing_or_refused_root_is_not_edited_again() {
        let done = ShownStatus::Ended(Ending::Completed);
        let cases = [
            (
                RootFailure::Missing("message_not_found".into()),
                Finished::Missing,
            ),
            (
                RootFailure::Refused("missing_scope".into()),
                Finished::Refused,
            ),
        ];
        for (failure, want) in cases {
            let mut roots = Roots::default();
            roots.want(&work(), "t", "1.1".into(), face("title", done));
            let sent = roots.ready(true);
            assert_eq!(roots.finish(&sent[0], &Err(failure)), want);
            assert!(roots.ready(true).is_empty(), "{want:?}");
            assert_eq!(roots.wanted("work", "1.1"), None, "{want:?}");
        }
    }

    /// Activity of a pane with no thread is only remembered, for the root a
    /// later message starts; a pane with a thread wants its root updated.
    #[test]
    fn only_a_pane_with_a_thread_updates_a_root() {
        let mut threads = Threads::load();
        let pane = pane(3);
        let shown = threads
            .note_activity(pane, PaneActivity::Working, NOW)
            .unwrap();
        assert_eq!(
            threads.note_activity(pane, PaneActivity::Working, NOW + 9),
            None,
            "unchanged"
        );
        threads.want_for_pane(pane, shown);
        assert_eq!(threads.roots.wanted("work", "1.1"), None, "no thread yet");

        let claude = conversation("claude", "s1");
        threads.place(&work(), pane, &claude, NOW);
        threads
            .titles
            .insert(("work".into(), pane), claude.title.clone());
        threads.remember(&work(), "t1".into(), claude.key.clone(), "1.1".into(), NOW);
        threads.want_for_pane(pane, shown);
        assert_eq!(
            threads.roots.wanted("work", "1.1"),
            Some(face(&claude.title, shown))
        );
    }
}

/// The host's reports reach a pane's root, including on a connection that
/// cannot send right now; a closed pane is told so, and its place is
/// forgotten while its `Closed` status is kept for a root still landing.
#[gpui::test]
fn reported_activity_reaches_the_panes_root_until_it_closes(cx: &mut gpui::TestAppContext) {
    use super::roots::RootFace;
    use super::status::{PaneActivity, ShownStatus};
    use crate::remote_channel::global::RemoteChannels;
    cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let channels = cx.global_mut::<RemoteChannels>();
        let connection = &channels.connections["work"];
        let scope = driver::scope(
            "work",
            connection.bot.as_ref().unwrap(),
            connection.config.recipient.as_ref().unwrap(),
        );
        let pane = pane(4);
        let claude = conversation("claude", "s1");
        let threads = &mut channels.threads;
        threads.place(&scope, pane, &claude, NOW);
        threads
            .titles
            .insert(("work".into(), pane), claude.title.clone());
        threads.remember(&scope, "t1".into(), claude.key.clone(), "1.1".into(), NOW);
        // Not sending now does not lose the update; the retry tick sends it.
        channels.connections.get_mut("work").unwrap().bot = None;

        super::roots::pane_activity(pane, PaneActivity::Working, cx);
        let wanted = |cx: &mut gpui::App| {
            cx.global::<RemoteChannels>()
                .threads
                .roots
                .wanted("work", "1.1")
                .map(|face: RootFace| face.shown)
        };
        assert!(matches!(wanted(cx), Some(ShownStatus::Working { .. })));

        super::roots::pane_activity(pane, PaneActivity::Closed, cx);
        assert_eq!(wanted(cx), Some(ShownStatus::Closed));
        let threads = &cx.global::<RemoteChannels>().threads;
        assert_eq!(threads.activity.get(&pane), Some(&ShownStatus::Closed));
        assert!(!threads.last.keys().any(|(_, held)| *held == pane));
        assert!(!threads.titles.keys().any(|(_, held)| *held == pane));
    });
}

/// The defect this guards: after a restart this run did not know what a root
/// showed, so a root left "working" stayed so until the status changed
/// again. A post into an existing thread now brings its root up to date.
#[test]
fn a_post_into_an_existing_thread_brings_its_root_up_to_date() {
    use super::roots::RootFace;
    use super::status::{Ending, PaneActivity, ShownStatus};
    let mut threads = Threads::load();
    let claude = conversation("claude", "s1");
    // A previous run left this thread; this run knows nothing of its root.
    threads.remember(&work(), "t1".into(), claude.key.clone(), "1.1".into(), NOW);
    let pane = pane(5);
    let ended = PaneActivity::Ended(Ending::Completed);
    threads.note_activity(pane, ended, NOW).unwrap();
    assert_eq!(threads.roots.wanted("work", "1.1"), None, "not posted yet");

    let (root, _) = threads.place(&work(), pane, &claude, NOW);
    threads.want_root_after_place(&work(), pane, root.as_deref(), &claude);
    assert_eq!(
        threads.roots.wanted("work", "1.1"),
        Some(RootFace {
            title: claude.title.clone(),
            shown: ShownStatus::Ended(Ending::Completed),
        })
    );
}

/// The defect this guards: after `/clear` or a switch, the pane's thread did
/// not list its new conversation until the pane next posted, so replies and
/// stop taps in the thread were refused while the pane was open and running.
#[gpui::test]
fn a_pane_that_moved_to_a_new_conversation_still_answers_in_its_thread(
    cx: &mut gpui::TestAppContext,
) {
    use super::route::{ThreadReply, route_reply};
    use crate::remote_channel::global::RemoteChannels;
    let fixture = crate::test_support::workspace_with_agent_chat(cx);
    let pane_id = fixture.pane();
    let key = fixture.workspace.update(cx, |ws, cx| {
        ws.set_pane_session_for_test(pane_id, "s1", cx);
        ws.remote_conversation(pane_id, cx).unwrap()
    });
    let pane = fixture.pane_ref(cx);
    let bot = cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let channels = cx.global_mut::<RemoteChannels>();
        let connection = &channels.connections["work"];
        let bot = connection.bot.clone().unwrap();
        let scope = driver::scope("work", &bot, connection.config.recipient.as_ref().unwrap());
        channels.threads.place(&scope, pane, &key, NOW);
        channels
            .threads
            .remember(&scope, "t1".into(), key.key.clone(), "1.1".into(), NOW);
        bot
    });
    fixture
        .workspace
        .update(cx, |ws, cx| ws.set_pane_session_for_test(pane_id, "s2", cx));
    cx.update(|cx| {
        assert_eq!(
            route_reply("work", &bot, Some("1.1"), cx),
            ThreadReply::Pane(pane)
        );
    });
}
