use super::super::roots::{Finished, RootFace, Roots};
use super::super::state::Threads;
use super::super::status::{Ending, PaneActivity, ShownStatus};
use super::*;
use crate::remote_channel::global::RemoteChannels;
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
    assert_eq!(
        threads.roots_ref().wanted("work", "1.1"),
        None,
        "no thread yet"
    );

    let claude = conversation("claude", "s1");
    threads.place(&work(), pane, &claude, NOW);
    threads.want_root_after_place(&work(), pane, None, &claude);
    threads.remember(&work(), "t1".into(), claude.key.clone(), "1.1".into(), NOW);
    threads.want_for_pane(pane, shown);
    assert_eq!(
        threads.roots_ref().wanted("work", "1.1"),
        Some(face(&claude.title, shown))
    );
}

/// The host's reports reach a pane's root, including on a connection that
/// cannot send right now; a closed pane is told so, and its place is
/// forgotten while its `Closed` status is kept for a root still landing.
#[gpui::test]
fn reported_activity_reaches_the_panes_root_until_it_closes(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let _outgoing = crate::remote_channel::global::tests::setup(cx);
        let channels = cx.global_mut::<RemoteChannels>();
        let connection = &channels.connections["work"];
        let scope = Scope::serving(
            "work",
            connection.bot.as_ref().unwrap(),
            connection.config.recipient.as_ref().unwrap(),
        );
        let pane = pane(4);
        let claude = conversation("claude", "s1");
        let threads = &mut channels.threads;
        threads.place(&scope, pane, &claude, NOW);
        threads.want_root_after_place(&scope, pane, None, &claude);
        threads.remember(&scope, "t1".into(), claude.key.clone(), "1.1".into(), NOW);
        // Not sending now does not lose the update; the retry tick sends it.
        channels.connections.get_mut("work").unwrap().bot = None;

        super::super::roots::pane_activity(pane, PaneActivity::Working, cx);
        let wanted = |cx: &mut gpui::App| {
            cx.global::<RemoteChannels>()
                .threads
                .roots_ref()
                .wanted("work", "1.1")
                .map(|face: RootFace| face.shown)
        };
        assert!(matches!(wanted(cx), Some(ShownStatus::Working { .. })));

        super::super::roots::pane_activity(pane, PaneActivity::Closed, cx);
        assert_eq!(wanted(cx), Some(ShownStatus::Closed));
        let threads = &cx.global::<RemoteChannels>().threads;
        assert_eq!(threads.activity_of(pane), Some(ShownStatus::Closed));
        assert!(!threads.knows_pane(pane), "its place is forgotten");
    });
}
