//! Unsent review text is kept: leaving the detail, reopening it and a
//! restart (the store read again from disk) find it where it was left.

use daruda_github::availability::{CredentialSource, GhAccount};
use daruda_github::{ReviewComment, ReviewThread, ReviewThreads};
use daruda_store::review_drafts::load_review_drafts_in;
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use super::pull_request_detail::{item, mergeable, open, repo, window};
use crate::github::DraftStore;
use crate::workspace::pages::pull_requests::detail::PrTab;

/// The detail of #7 as `detail`, with `me` signed in on the host before
/// anything asks `gh`: drafts are kept under that account.
fn opened_signed_in(
    cx: &mut TestAppContext,
    detail: daruda_github::PrDetail,
) -> (
    super::GitFixture,
    daruda_github::fake::FakeGh,
    crate::workspace::pages::pull_requests::detail::PrDetailId,
) {
    let (f, gh, _) = window(cx);
    gh.set_account(
        "github.com",
        GhAccount {
            login: "me".into(),
            source: CredentialSource::Env("GH_TOKEN".into()),
        },
    );
    // The account is learned the way the page learns it: `gh` asked again.
    f.ws.update(cx, |ws, cx| ws.recheck_gh("github.com", cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    gh.set_pr_detail(&repo(), 7, detail);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id = shown(&f, cx);
    (f, gh, id)
}

fn shown(
    f: &super::GitFixture,
    cx: &mut TestAppContext,
) -> crate::workspace::pages::pull_requests::detail::PrDetailId {
    f.ws.read_with(cx, |ws, _| {
        ws.pages.pull_requests.detail.as_ref().unwrap().id()
    })
}

fn leave(f: &super::GitFixture, vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.back_to_pr_list(window, cx)));
    vcx.run_until_parked();
}

fn shown_id(
    f: &super::GitFixture,
    vcx: &mut VisualTestContext,
) -> crate::workspace::pages::pull_requests::detail::PrDetailId {
    f.ws.read_with(vcx, |ws, _| {
        ws.pages.pull_requests.detail.as_ref().unwrap().id()
    })
}

#[gpui::test]
fn a_description_draft_survives_leaving_and_a_restart(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.body = "Before".into();
    let (f, _, id) = opened_signed_in(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.edit_pr_body(id, window, cx)));
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let edit = ws
                .pages
                .pull_requests
                .detail
                .as_ref()
                .unwrap()
                .body_edit()
                .unwrap();
            let input = edit.input.clone();
            input.update(cx, |input, cx| input.set_value("Half written", window, cx));
        })
    });
    vcx.run_until_parked();
    leave(&f, &mut vcx);
    let saved = load_review_drafts_in(dir.path()).unwrap();
    let description = saved.prs[0].description.clone().expect("on disk once left");
    assert_eq!(
        (description.base.as_str(), description.text.as_str()),
        ("Before", "Half written")
    );
    assert_eq!(saved.prs[0].of.account, "me");

    // A restart reads the drafts again from disk.
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    open(&f, item(7, "feat"), &mut vcx);
    vcx.run_until_parked();
    let again = shown_id(&f, &mut vcx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(again, PrTab::Description, cx)
    });
    vcx.run_until_parked();
    let edit = vcx.debug_bounds("pr-body-edit").unwrap().center();
    vcx.simulate_click(edit, Modifiers::default());
    vcx.run_until_parked();
    let text = vcx.update(|_, cx| {
        f.ws.read_with(cx, |ws, cx| {
            let edit = ws
                .pages
                .pull_requests
                .detail
                .as_ref()
                .unwrap()
                .body_edit()
                .unwrap();
            edit.input.read(cx).value().to_string()
        })
    });
    assert_eq!(text, "Half written", "continued where it was left");

    let cancel = vcx.debug_bounds("pr-body-cancel").unwrap().center();
    vcx.simulate_click(cancel, Modifiers::default());
    vcx.run_until_parked();
    leave(&f, &mut vcx);
    let saved = load_review_drafts_in(dir.path()).unwrap();
    assert!(saved.prs.is_empty(), "cancelled: the draft is discarded");
}

#[gpui::test]
fn a_reply_draft_is_marked_continued_and_dropped_once_posted(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let (f, gh, id) = opened_signed_in(cx, mergeable(7, "feat"));
    gh.set_review_threads(
        &repo(),
        7,
        ReviewThreads {
            threads: vec![ReviewThread {
                id: "t1".into(),
                path: "a.rs".into(),
                comments: vec![ReviewComment {
                    id: "c1".into(),
                    body: "Why?".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            truncated: false,
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_pr_reply(id, "t1", window, cx))
    });
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
            input.update(cx, |input, cx| input.set_value("Because.", window, cx));
        })
    });
    vcx.run_until_parked();
    leave(&f, &mut vcx);
    assert_eq!(
        load_review_drafts_in(dir.path()).unwrap().prs[0].replies[0].text,
        "Because."
    );

    open(&f, item(7, "feat"), &mut vcx);
    vcx.run_until_parked();
    let again = shown_id(&f, &mut vcx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(again, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_pr_reply(again, "t1", window, cx))
    });
    let text = vcx.update(|_, cx| {
        f.ws.read_with(cx, |ws, cx| {
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
            let reply = detail
                .thread_work()
                .get("t1")
                .unwrap()
                .reply
                .clone()
                .unwrap();
            reply.read(cx).value().to_string()
        })
    });
    assert_eq!(text, "Because.");
    f.ws.update(&mut vcx, |ws, cx| ws.post_pr_reply(again, "t1", cx));
    vcx.run_until_parked();
    leave(&f, &mut vcx);
    assert!(
        load_review_drafts_in(dir.path()).unwrap().prs.is_empty(),
        "posted: dropped"
    );
}

/// A reply that fails after its detail is gone is held as sent with its
/// draft, across a restart: reopened, it is checked rather than sent again,
/// and the read that finds it posted drops the draft.
#[gpui::test]
fn a_reply_failed_with_its_detail_gone_is_checked_not_sent_again(cx: &mut TestAppContext) {
    use daruda_github::fake::{GhCall, GhFailure, GhOp};
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let (f, gh, id) = opened_signed_in(cx, mergeable(7, "feat"));
    let threads = |comments: Vec<ReviewComment>| ReviewThreads {
        threads: vec![ReviewThread {
            id: "t1".into(),
            path: "a.rs".into(),
            comments,
            ..Default::default()
        }],
        truncated: false,
    };
    let asked = ReviewComment {
        id: "c1".into(),
        body: "Why?".into(),
        ..Default::default()
    };
    gh.set_review_threads(&repo(), 7, threads(vec![asked.clone()]));
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_pr_reply(id, "t1", window, cx);
            let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
            let input = detail
                .thread_work()
                .get("t1")
                .unwrap()
                .reply
                .clone()
                .unwrap();
            input.update(cx, |input, cx| input.set_value("Because.", window, cx));
        })
    });
    gh.fail(GhOp::ReplyToThread, GhFailure::Timeout);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.post_pr_reply(id, "t1", cx);
            ws.back_to_pr_list(window, cx);
        })
    });
    vcx.run_until_parked();
    let saved = load_review_drafts_in(dir.path()).unwrap();
    let sent = saved.prs[0].replies[0]
        .sent
        .clone()
        .expect("held as sent, written at once");
    assert_eq!(
        (
            sent.body.as_str(),
            sent.after.as_deref(),
            sent.by.as_deref(),
            sent.cut_off
        ),
        ("Because.", Some("c1"), Some("me"), true)
    );

    // A restart reads it back; GitHub posted it after all.
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    gh.clear_failure(GhOp::ReplyToThread);
    gh.set_review_threads(
        &repo(),
        7,
        threads(vec![
            asked,
            ReviewComment {
                id: "c2".into(),
                author: Some("me".into()),
                body: "Because.".into(),
                ..Default::default()
            },
        ]),
    );
    open(&f, item(7, "feat"), &mut vcx);
    vcx.run_until_parked();
    let again = shown_id(&f, &mut vcx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(again, PrTab::Threads, cx)
    });
    vcx.run_until_parked();
    let calls = gh.calls().len();
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.open_pr_reply(again, "t1", window, cx);
            ws.post_pr_reply(again, "t1", cx);
        })
    });
    vcx.run_until_parked();
    assert!(
        !gh.calls()[calls..]
            .iter()
            .any(|call| matches!(call, GhCall::ReplyToThread { .. })),
        "not sent again"
    );
    let closed = f.ws.read_with(&vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        detail
            .thread_work()
            .get("t1")
            .is_none_or(|work| work.reply.is_none() && work.op.is_none())
    });
    assert!(closed, "found posted: the box closes");
    leave(&f, &mut vcx);
    assert!(
        load_review_drafts_in(dir.path()).unwrap().prs.is_empty(),
        "and its draft is done with"
    );
}

#[gpui::test]
fn without_a_known_account_the_editor_says_its_text_is_not_kept(cx: &mut TestAppContext) {
    use crate::github::DraftKeeping;
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    let (f, _, id) = super::pull_request_detail::opened(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| ws.edit_pr_body(id, window, cx)));
    vcx.run_until_parked();
    let keeping = f.ws.update(&mut vcx, |ws, cx| {
        ws.stage_pr_page(cx);
        let snap = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
        match &snap.body {
            crate::workspace::pages::pull_requests::detail::view::DetailBody::Ready(ready) => {
                ready.discussion.as_ref().unwrap().keeping
            }
            other => panic!("{other:?}"),
        }
    });
    assert_eq!(
        keeping,
        DraftKeeping::NoAccount,
        "said, not silently dropped"
    );
}

#[gpui::test]
fn drafts_for_unlisted_threads_are_not_called_orphans_when_the_list_is_cut_short(
    cx: &mut TestAppContext,
) {
    use crate::workspace::pages::pull_requests::detail::view::{DetailBody, ThreadsView};
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let (f, gh, id) = opened_signed_in(cx, mergeable(7, "feat"));
    let owner = cx.update(|cx| DraftStore::owner(&repo(), 7, cx)).unwrap();
    cx.update(|cx| DraftStore::set_reply(&owner, "far-thread", Some("Kept".into()), cx));
    let orphans = |truncated: bool, f: &super::GitFixture, cx: &mut TestAppContext| {
        gh.set_review_threads(
            &repo(),
            7,
            ReviewThreads {
                threads: Vec::new(),
                truncated,
            },
        );
        f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
        f.ws.update(cx, |ws, cx| ws.select_pr_detail_tab(id, PrTab::Threads, cx));
        cx.run_until_parked();
        f.ws.update(cx, |ws, cx| {
            ws.stage_pr_page(cx);
            let snap = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
            let DetailBody::Ready(ready) = &snap.body else {
                panic!("read");
            };
            match &ready.threads {
                ThreadsView::Listed { orphan_drafts, .. } => orphan_drafts.len(),
                other => panic!("{other:?}"),
            }
        })
    };
    assert_eq!(
        orphans(true, &f, cx),
        0,
        "the thread may be past the threads read"
    );
    assert_eq!(orphans(false, &f, cx), 1, "every thread read: it is gone");
}

#[gpui::test]
fn a_line_comment_is_kept_as_typed_with_its_lines_and_head(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::diff_layout::Side;
    use daruda_github::{FileChange, PrFile, PrFiles};
    use daruda_store::review_drafts::DraftSide;
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1".into();
    let (f, gh, id) = opened_signed_in(cx, detail.clone());
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![PrFile {
                path: "a.rs".into(),
                change: FileChange::Modified,
                additions: 2,
                deletions: 0,
                patch: Some("@@ -1,1 +1,3 @@\n one\n+two\n+three".into()),
                blob: Some("b".into()),
            }],
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_line(id, Side::New, 2, false, cx);
        ws.select_pr_line(id, Side::New, 3, true, cx);
    });
    vcx.run_until_parked();
    let comment = vcx.debug_bounds("pr-lines-comment").unwrap().center();
    vcx.simulate_click(comment, Modifiers::default());
    vcx.run_until_parked();
    let suggest = vcx.debug_bounds("pr-composer-suggest").unwrap().center();
    vcx.simulate_click(suggest, Modifiers::default());
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let composer = ws
                .pages
                .pull_requests
                .detail
                .as_ref()
                .unwrap()
                .composer()
                .unwrap();
            let body = composer.body.clone();
            body.update(cx, |body, cx| body.set_value("Fold these.", window, cx));
        })
    });
    vcx.run_until_parked();
    let done = vcx.debug_bounds("pr-composer-done").unwrap().center();
    vcx.simulate_click(done, Modifiers::default());
    vcx.run_until_parked();
    leave(&f, &mut vcx);
    let saved = load_review_drafts_in(dir.path()).unwrap();
    let kept = &saved.prs[0].comments[0];
    assert_eq!(
        (
            kept.path.as_str(),
            kept.side,
            kept.start,
            kept.end,
            kept.revision.as_str(),
            kept.body.as_str(),
            kept.suggestion.as_deref()
        ),
        (
            "a.rs",
            DraftSide::Right,
            2,
            3,
            "4f9c2e1",
            "Fold these.",
            Some("two\nthree")
        ),
        "the suggestion starts from the selected lines"
    );

    // Reopened on a new head: the comment is still pending, marked as
    // written on the earlier one.
    gh.set_pr_detail(
        &repo(),
        7,
        daruda_github::PrDetail {
            head_sha: "9a8b7c6".into(),
            ..detail
        },
    );
    open(&f, item(7, "feat"), &mut vcx);
    vcx.run_until_parked();
    let again = shown_id(&f, &mut vcx);
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(again, PrTab::Files, cx)
    });
    vcx.run_until_parked();
    let pending = f.ws.update(&mut vcx, |ws, cx| {
        ws.stage_pr_page(cx);
        let snap = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
        let crate::workspace::pages::pull_requests::detail::view::DetailBody::Ready(ready) =
            &snap.body
        else {
            panic!("read");
        };
        ready.review_comments.as_ref().unwrap().pending.clone()
    });
    assert_eq!(pending.len(), 1);
    assert!(pending[0].outdated, "{pending:?}");
    let discard = vcx.debug_bounds("pr-pending-discard-1").unwrap().center();
    vcx.simulate_click(discard, Modifiers::default());
    vcx.run_until_parked();
    leave(&f, &mut vcx);
    assert!(load_review_drafts_in(dir.path()).unwrap().prs.is_empty());
}

#[gpui::test]
fn a_suggestion_starts_from_the_comments_own_unfolded_lines(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::diff_layout::Side;
    use crate::workspace::pages::pull_requests::detail::files::Unchanged;
    use daruda_github::{FileChange, PrFile, PrFiles};
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "4f9c2e1".into();
    let (f, gh, id) = opened_signed_in(cx, detail);
    let file = |path: &str, patch: &str| PrFile {
        path: path.into(),
        change: FileChange::Modified,
        additions: 2,
        deletions: 0,
        patch: Some(patch.into()),
        blob: Some(path.into()),
    };
    gh.set_pr_files(
        &repo(),
        7,
        PrFiles {
            files: vec![
                file("a.rs", "@@ -1,3 +1,5 @@\n keep\n+one\n mid\n+two\n tail"),
                file("b.rs", "@@ -1,1 +1,2 @@\n x\n+y"),
            ],
        },
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    f.ws.update(&mut vcx, |ws, cx| ws.refresh_pr_detail(id, cx));
    vcx.run_until_parked();
    f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Files, cx);
    });
    vcx.run_until_parked();
    // Unchanged lines folded away: the selection still spans "mid".
    f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_diff_unchanged(id, Unchanged::Folded, cx);
        ws.select_pr_line(id, Side::New, 2, false, cx);
        ws.select_pr_line(id, Side::New, 4, true, cx);
    });
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.comment_pr_lines(id, window, cx))
    });
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.toggle_pr_suggestion(id, window, cx))
    });
    vcx.run_until_parked();
    let suggestion = |f: &super::GitFixture, vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            f.ws.read_with(cx, |ws, cx| {
                let composer = ws.pages.pull_requests.detail.as_ref().unwrap().composer()?;
                Some(composer.suggestion.as_ref()?.read(cx).value().to_string())
            })
        })
    };
    assert_eq!(
        suggestion(&f, &mut vcx).as_deref(),
        Some("one\nmid\ntwo"),
        "every line, the folded one too"
    );
    f.ws.update(&mut vcx, |ws, cx| ws.close_pr_line_comment(id, cx));
    vcx.run_until_parked();

    // Editing the pending comment shows its file and lines, whatever was
    // selected meanwhile.
    f.ws.update(&mut vcx, |ws, cx| {
        ws.show_pr_file(id, "b.rs", cx);
        ws.select_pr_line(id, Side::New, 2, false, cx);
    });
    let comment = cx.update(|cx| {
        let owner = DraftStore::owner(&repo(), 7, cx).unwrap();
        DraftStore::of(&owner, cx).unwrap().comments[0].id
    });
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            ws.edit_pr_line_comment(id, comment, window, cx)
        })
    });
    vcx.run_until_parked();
    let selected = f.ws.read_with(&vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail.as_ref().unwrap();
        detail
            .files()
            .and_then(|files| files.selection().cloned())
            .map(|lines| (lines.path, lines.start, lines.end))
    });
    assert_eq!(selected, Some(("a.rs".into(), 2, 4)));
    assert_eq!(
        suggestion(&f, &mut vcx).as_deref(),
        Some("one\nmid\ntwo"),
        "kept as typed"
    );
}
