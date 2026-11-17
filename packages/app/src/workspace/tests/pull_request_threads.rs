//! The review-threads tab: a thread expanded in full, and the way from it to
//! its lines in the diff.

use daruda_github::{DiffSide, PrFile, PrFiles, ReviewComment, ReviewThread, ReviewThreads};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use super::pull_request_detail::{mergeable, opened, repo};
use crate::workspace::pages::pull_requests::detail::PrTab;
use crate::workspace::pages::pull_requests::detail::diff_layout::Side;

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
    gh.set_review_threads(7, threads());
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
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
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
    gh.set_review_threads(7, threads());
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
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
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
    gh.set_review_threads(7, threads());
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
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
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
    gh.fail(
        GhOp::ReplyToThread,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
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
                let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
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
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
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

#[gpui::test]
fn a_failed_reply_that_did_post_is_not_offered_again(cx: &mut TestAppContext) {
    use daruda_github::fake::{GhFailure, GhOp};
    let (f, gh, id) = opened(cx, mergeable(7, "feat"));
    gh.set_review_threads(7, threads());
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
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
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
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
    // gh times out, but GitHub took the reply: the next read has it.
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
    gh.set_review_threads(7, posted);
    let post = vcx.debug_bounds("pr-thread-post-0").unwrap().center();
    vcx.simulate_click(post, Modifiers::default());
    vcx.run_until_parked();
    let work = f.ws.read_with(&vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        detail
            .thread_work()
            .get("t1")
            .map(|work| (work.reply.is_some(), work.op.clone(), work.checking()))
    });
    assert_eq!(
        work,
        Some((false, None, false)),
        "found posted: the box closes, nothing to retry"
    );
    assert!(vcx.debug_bounds("pr-thread-post-0").is_none());
}
