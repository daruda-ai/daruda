//! The review-threads tab: a thread expanded in full, and the way from it to
//! its lines in the diff.

use daruda_github::{DiffSide, PrFile, PrFiles, ReviewComment, ReviewThread, ReviewThreads};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use daruda_github::fake::FakeGh;

use super::pull_request_detail::{mergeable, opened, repo};
use crate::workspace::pages::pull_requests::detail::diff_layout::Side;
use crate::workspace::pages::pull_requests::detail::thread_actions::ThreadOp;
use crate::workspace::pages::pull_requests::detail::{PrDetailId, PrTab};
use crate::workspace::tests::GitFixture;

fn comment(id: &str, author: &str, body: &str) -> ReviewComment {
    ReviewComment {
        id: id.into(),
        url: Some(format!("https://github.com/o/n/pull/7#discussion_{id}")),
        author: Some(author.into()),
        body: body.into(),
    }
}

fn threads() -> ReviewThreads {
    ReviewThreads {
        threads: vec![
            ReviewThread {
                id: "t1".into(),
                path: "a.rs".into(),
                lines: Some((2, 3)),
                side: DiffSide::Right,
                outdated: false,
                comments: vec![
                    comment("c1", "rev", "Rename **this**.\n\nSecond paragraph."),
                    comment("c2", "author", "Done."),
                ],
                more_comments: false,
                may: Default::default(),
            },
            ReviewThread {
                id: "t3".into(),
                path: "gone.rs".into(),
                lines: Some((1, 1)),
                comments: vec![comment("c4", "rev", "On a file no longer listed")],
                ..Default::default()
            },
            ReviewThread {
                id: "t2".into(),
                path: "b.rs".into(),
                lines: Some((9, 9)),
                outdated: true,
                comments: vec![comment("c3", "rev", "Stale")],
                ..Default::default()
            },
        ],
        truncated: false,
    }
}

#[gpui::test]
fn a_thread_expands_in_full_and_leads_to_its_lines(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1".into();
    let (f, gh, id) = opened(cx, detail);
    gh.set_review_threads(&repo(), 7, threads());
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![PrFile {
                path: "a.rs".into(),
                change: daruda_github::FileChange::Modified,
                additions: 2,
                deletions: 0,
                patch: Some("@@ -1,1 +1,3 @@\n one\n+two\n+three".into()),
                blob: Some("b".into()),
            }],
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-thread-full-0").is_none(),
        "folded at first"
    );

    let row = vcx.debug_bounds("pr-thread-0").unwrap().center();
    vcx.simulate_click(row, Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-thread-full-0").is_some());
    assert!(vcx.debug_bounds("pr-thread-diff-0").is_some());

    // An outdated thread is shown, but not placed on the current lines.
    let row = vcx.debug_bounds("pr-thread-2").unwrap().center();
    vcx.simulate_click(row, Modifiers::default());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-thread-full-2").is_some());
    assert!(vcx.debug_bounds("pr-thread-diff-2").is_none());

    let go = vcx.debug_bounds("pr-thread-diff-0").unwrap().center();
    vcx.simulate_click(go, Modifiers::default());
    vcx.run_until_parked();
    let selected = f.ws.read_with(&vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        assert_eq!(detail.tab(), PrTab::Files);
        detail
            .files()
            .and_then(|files| files.selection().cloned())
            .map(|lines| {
                (
                    lines.path,
                    lines.side,
                    lines.start,
                    lines.end,
                    lines.revision,
                )
            })
    });
    assert_eq!(
        selected,
        Some(("a.rs".into(), Side::New, 2, 3, "4f9c2e1".into())),
        "the thread's lines, selected at the commit read"
    );
    assert!(vcx.debug_bounds("pr-lines-selected").is_some());
}

#[gpui::test]
fn a_thread_folds_back_and_lines_in_an_unlisted_file_are_given_up(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_review_threads(&repo(), 7, threads());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    for _ in 0..2 {
        let row = vcx.debug_bounds("pr-thread-1").unwrap().center();
        vcx.simulate_click(row, Modifiers::default());
        vcx.run_until_parked();
    }
    assert!(
        vcx.debug_bounds("pr-thread-full-1").is_none(),
        "pressed twice: folded"
    );

    let target = f.ws.update(&mut vcx, |ws, cx| {
        ws.stage_pr_page(cx);
        let snap = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
        let crate::workspace::pages::pull_requests::detail::view::DetailBody::Ready(ready) =
            &snap.body
        else {
            panic!("read");
        };
        let crate::workspace::pages::pull_requests::detail::view::ThreadsView::Listed {
            rows, ..
        } = &ready.threads
        else {
            panic!("listed");
        };
        rows[1].target.clone().unwrap()
    });
    let file = |path: &str| PrFile {
        path: path.into(),
        change: daruda_github::FileChange::Modified,
        additions: 1,
        deletions: 0,
        patch: Some("@@ -1,1 +1,2 @@\n one\n+two".into()),
        blob: Some(path.into()),
    };
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file("a.rs")],
        },
    );
    f.ws.update(&mut vcx, |ws, cx| ws.show_pr_thread_in_diff(id, target, cx));
    vcx.run_until_parked();
    // A later read lists the file; the lines asked for before are not
    // selected on their own when it is shown.
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![file("a.rs"), file("gone.rs")],
        },
    );
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| ws.show_pr_file(id, "gone.rs", cx));
    let selected = f.ws.read_with(&vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        detail.files().and_then(|files| files.selection().cloned())
    });
    assert_eq!(
        selected, None,
        "gone.rs was not listed when asked: given up"
    );
}

#[gpui::test]
fn a_reply_posts_and_a_thread_resolves_each_on_its_own(cx: &mut TestAppContext) {
    use daruda_github::fake::{GhCall, GhFailure, GhOp};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_review_threads(&repo(), 7, threads());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    for row in ["pr-thread-0", "pr-thread-1"] {
        let at = vcx.debug_bounds(row).unwrap().center();
        vcx.simulate_click(at, Modifiers::default());
        vcx.run_until_parked();
    }

    // A reply that fails keeps its text and says why.
    let reply = vcx.debug_bounds("pr-thread-reply-0").unwrap().center();
    vcx.simulate_click(reply, Modifiers::default());
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let detail = ws.pages.pull_requests.detail().unwrap();
            let input = detail
                .thread_work()
                .get("t1")
                .unwrap()
                .reply
                .clone()
                .unwrap();
            input.update(cx, |input, cx| {
                input.set_value("Fixed in the next push.", window, cx)
            });
        })
    });
    // Refused outright: it did not post, so it may go again.
    gh.fail(
        GhOp::ReplyToThread,
        GhFailure::Exit {
            code: Some(1),
            stderr: "gh: Validation Failed (HTTP 422)".into(),
        },
    );
    // Meanwhile the other thread resolves.
    let resolve = vcx.debug_bounds("pr-thread-resolve-1").unwrap().center();
    vcx.simulate_click(resolve, Modifiers::default());
    let post = vcx.debug_bounds("pr-thread-post-0").unwrap().center();
    vcx.simulate_click(post, Modifiers::default());
    vcx.run_until_parked();
    let state = |f: &super::GitFixture, vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            f.ws.read_with(cx, |ws, cx| {
                let detail = ws.pages.pull_requests.detail().unwrap();
                let work = detail.thread_work();
                let text = work
                    .get("t1")
                    .and_then(|w| w.reply.as_ref())
                    .map(|reply| reply.read(cx).value().to_string());
                let listed = match detail.threads() {
                    Some(crate::workspace::pages::pull_requests::detail::Load::Ready(read)) => {
                        read.threads.iter().any(|thread| thread.id == "t3")
                    }
                    _ => true,
                };
                (
                    text,
                    work.get("t1").and_then(|w| w.op.clone()),
                    !listed || work.resolved_here("t3"),
                )
            })
        })
    };
    let (text, op, resolved) = state(&f, &mut vcx);
    assert_eq!(
        text.as_deref(),
        Some("Fixed in the next push."),
        "kept for another try"
    );
    assert!(
        matches!(
            op,
            Some(
                crate::workspace::pages::pull_requests::detail::thread_actions::ThreadOp::Failed(
                    ..
                )
            )
        ),
        "{op:?}"
    );
    // The failure read the threads again: the resolved one is gone.
    assert!(resolved, "the other thread's action stands on its own");
    assert!(
        gh.calls().iter().any(|call| matches!(call, GhCall::SetThreadResolved { thread, resolved: true, .. } if thread == "t3")),
    );

    // Tried again, it posts: the box closes and the reply is in the thread.
    gh.clear_failure(GhOp::ReplyToThread);
    let post = vcx.debug_bounds("pr-thread-post-0").unwrap().center();
    vcx.simulate_click(post, Modifiers::default());
    vcx.run_until_parked();
    let (text, op, _) = state(&f, &mut vcx);
    assert_eq!((text, op), (None, None));
    let replies = f.ws.read_with(&vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        match detail.threads() {
            Some(crate::workspace::pages::pull_requests::detail::Load::Ready(threads)) => threads
                .threads
                .iter()
                .find(|thread| thread.id == "t1")
                .map(|thread| thread.comments.len()),
            _ => None,
        }
    });
    assert_eq!(replies, Some(3), "two comments, then the reply");
}

/// A reply typed under the first thread whose post times out although
/// GitHub took it: the next threads read has it.
fn type_a_reply_github_takes_silently(
    f: &GitFixture,
    gh: &FakeGh,
    id: PrDetailId,
    vcx: &mut VisualTestContext,
) {
    use daruda_github::fake::{GhFailure, GhOp};
    gh.set_review_threads(&repo(), 7, threads());
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    let row = vcx.debug_bounds("pr-thread-0").unwrap().center();
    vcx.simulate_click(row, Modifiers::default());
    vcx.run_until_parked();
    let reply = vcx.debug_bounds("pr-thread-reply-0").unwrap().center();
    vcx.simulate_click(reply, Modifiers::default());
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let detail = ws.pages.pull_requests.detail().unwrap();
            let input = detail
                .thread_work()
                .get("t1")
                .unwrap()
                .reply
                .clone()
                .unwrap();
            input.update(cx, |input, cx| {
                input.set_value("Posted after all.", window, cx)
            });
        })
    });
    gh.fail(
        GhOp::ReplyToThread,
        GhFailure::Exit {
            code: None,
            stderr: "timed out".into(),
        },
    );
    let mut posted = threads();
    posted.threads[0]
        .comments
        .push(comment("c9", "fake-viewer", "Posted after all."));
    gh.set_review_threads(&repo(), 7, posted);
}

/// The reply's box and what is still to do with it.
fn reply_work(f: &GitFixture, vcx: &VisualTestContext) -> Option<(bool, Option<ThreadOp>)> {
    f.ws.read_with(vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        detail
            .thread_work()
            .get("t1")
            .map(|work| (work.reply.is_some(), work.op.clone()))
    })
}

#[gpui::test]
fn a_failed_reply_that_did_post_is_not_offered_again(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    type_a_reply_github_takes_silently(&f, &gh, id, &mut vcx);
    let post = vcx.debug_bounds("pr-thread-post-0").unwrap().center();
    vcx.simulate_click(post, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        reply_work(&f, &vcx),
        Some((false, None)),
        "found posted: the box closes, nothing to retry"
    );
    assert!(vcx.debug_bounds("pr-thread-post-0").is_none());
}

/// A failed reply whose check cannot read the threads holds Post; the person
/// looking on GitHub can say it did not post, and its text may go again.
#[gpui::test]
fn an_unconfirmed_reply_is_held_until_the_person_says_it_did_not_post(cx: &mut TestAppContext) {
    use daruda_github::fake::{GhFailure, GhOp};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    type_a_reply_github_takes_silently(&f, &gh, id, &mut vcx);
    gh.fail(GhOp::ReviewThreads, GhFailure::Timeout);
    let post = vcx.debug_bounds("pr-thread-post-0").unwrap().center();
    vcx.simulate_click(post, Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(
            reply_work(&f, &vcx),
            Some((true, Some(ThreadOp::Unconfirmed { .. })))
        ),
        "{:?}",
        reply_work(&f, &vcx)
    );
    vcx.simulate_click(post, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        gh.calls()
            .iter()
            .filter(|call| matches!(call, daruda_github::fake::GhCall::ReplyToThread { .. }))
            .count(),
        1,
        "Post is held"
    );
    let not_posted = vcx.debug_bounds("pr-thread-not-posted-0").unwrap().center();
    vcx.simulate_click(not_posted, Modifiers::default());
    vcx.run_until_parked();
    assert!(
        matches!(
            reply_work(&f, &vcx),
            Some((true, Some(ThreadOp::Failed(..))))
        ),
        "the text kept, to go again"
    );
    assert!(vcx.debug_bounds("pr-thread-not-posted-0").is_none());
}

/// Threads dropped while hidden still settle a failed reply: the read made
/// when they are shown again finds it posted.
#[gpui::test]
fn a_failed_reply_is_checked_when_its_threads_are_shown_again(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    type_a_reply_github_takes_silently(&f, &gh, id, &mut vcx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.post_pr_reply(id, "t1", cx);
        ws.select_pr_detail_tab(id, PrTab::Description, cx);
        ws.refresh_pr_detail(id, cx);
    });
    vcx.run_until_parked();
    let held = f.ws.read_with(&vcx, |ws, _| {
        ws.pages.pull_requests.detail().unwrap().threads().is_some()
    });
    assert!(!held, "hidden threads are dropped by the refresh");
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        reply_work(&f, &vcx),
        Some((false, None)),
        "found posted: the box closes, nothing to retry"
    );
}

/// A failed refresh keeps the threads shown, and with them the mark of one
/// resolved here.
#[gpui::test]
fn a_resolve_stays_shown_when_the_threads_fail_to_read_again(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_review_threads(&repo(), 7, threads());
    f.ws.update(cx, |ws, cx| ws.select_pr_detail_tab(id, PrTab::Threads, cx));
    cx.run_until_parked();
    f.ws.update(cx, |ws, cx| ws.set_pr_thread_resolved(id, "t3", true, cx));
    cx.run_until_parked();
    let reads = gh.count(daruda_github::fake::GhOp::ReviewThreads);
    gh.fail(
        daruda_github::fake::GhOp::ReviewThreads,
        daruda_github::fake::GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
    assert_eq!(
        gh.count(daruda_github::fake::GhOp::ReviewThreads),
        reads + 1,
        "read again"
    );
    f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        // The threads read before the resolve stay up, so its mark does too.
        assert!(matches!(
            detail.threads(),
            Some(crate::workspace::pages::pull_requests::detail::Load::Ready(
                _
            ))
        ));
        assert!(detail.thread_work().resolved_here("t3"));
    });
}

/// A refresh reads the shown threads again in place: they stay up until the
/// answer, as every tab read on demand does.
#[gpui::test]
fn a_refresh_keeps_the_shown_threads_up(cx: &mut TestAppContext) {
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_review_threads(&repo(), 7, threads());
    f.ws.update(cx, |ws, cx| ws.select_pr_detail_tab(id, PrTab::Threads, cx));
    cx.run_until_parked();
    let shown = |cx: &mut TestAppContext| {
        f.ws.read_with(cx, |ws, _| {
            let detail = ws.pages.pull_requests.detail().unwrap();
            matches!(
                detail.threads(),
                Some(crate::workspace::pages::pull_requests::detail::Load::Ready(
                    _
                ))
            )
        })
    };
    assert!(shown(cx));
    let reads = gh.count(daruda_github::fake::GhOp::ReviewThreads);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    assert!(shown(cx), "still shown while read again");
    cx.run_until_parked();
    assert!(shown(cx));
    assert_eq!(
        gh.count(daruda_github::fake::GhOp::ReviewThreads),
        reads + 1,
        "read again"
    );
}
