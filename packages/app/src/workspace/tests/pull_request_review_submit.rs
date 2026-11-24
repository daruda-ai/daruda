//! Submitting a review: what is sent, what is held back, and that only
//! what GitHub took leaves the drafts — even when the answer is lost.

use daruda_github::availability::{CredentialSource, GhAccount};
use daruda_github::fake::{GhCall, GhFailure, GhOp};
use daruda_github::{FileChange, PrFile, PrFiles, ReviewEvent};
use daruda_store::review_drafts::{DraftSide, LineCommentDraft, load_review_drafts_in};
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use super::pull_request_detail::{item, mergeable, open, repo, window};
use crate::github::DraftStore;
use crate::workspace::pages::pull_requests::detail::review_submit::{SubmitBlock, SubmitState};

struct Fixture {
    f: super::GitFixture,
    gh: daruda_github::fake::FakeGh,
    id: crate::workspace::pages::pull_requests::detail::PrDetailId,
    dir: tempfile::TempDir,
}

/// #7 by `alice` at head `h1`, viewed by `me`, a.rs changed, with one
/// comment written at h1 and one at an earlier head.
fn reviewing(cx: &mut TestAppContext) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let (f, gh, _) = window(cx);
    gh.set_account(
        "github.com",
        GhAccount {
            login: "me".into(),
            source: CredentialSource::Env("GH_TOKEN".into()),
        },
    );
    f.ws.update(cx, |ws, cx| ws.recheck_gh("github.com", cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "h1".into();
    detail.author = Some("alice".into());
    gh.set_pr_detail(&repo(), 7, detail);
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
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id =
        f.ws.read_with(cx, |ws, _| ws.pages.pull_requests.detail().unwrap().id());
    let owner = cx.update(|cx| DraftStore::owner(&repo(), 7, cx)).unwrap();
    for (revision, body) in [("h1", "Fold these."), ("h0", "Stale.")] {
        cx.update(|cx| {
            DraftStore::put_comment(
                &owner,
                None,
                LineCommentDraft {
                    id: 0,
                    path: "a.rs".into(),
                    side: DraftSide::Right,
                    start: 2,
                    end: 3,
                    revision: revision.into(),
                    body: body.into(),
                    suggestion: None,
                },
                cx,
            )
        });
    }
    Fixture { f, gh, id, dir }
}

fn state(t: &Fixture, vcx: &mut VisualTestContext) -> Option<SubmitState> {
    t.f.ws.read_with(vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        detail.review().map(|review| review.state.clone())
    })
}

fn submits(gh: &daruda_github::fake::FakeGh) -> Vec<(String, ReviewEvent, usize)> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::SubmitReview {
                commit,
                event,
                threads,
                ..
            } => Some((commit, event, threads.len())),
            _ => None,
        })
        .collect()
}

fn begin(t: &Fixture, vcx: &mut VisualTestContext, text: &str) {
    vcx.run_until_parked();
    let open_review = vcx.debug_bounds("pr-review-open").unwrap().center();
    vcx.simulate_click(open_review, Modifiers::default());
    vcx.update(|window, cx| {
        t.f.ws.update(cx, |ws, cx| {
            let body = ws
                .pages
                .pull_requests
                .detail()
                .unwrap()
                .review()
                .unwrap()
                .body
                .clone();
            body.update(cx, |body, cx| body.set_value(text.to_owned(), window, cx));
        })
    });
    vcx.run_until_parked();
}

#[gpui::test]
fn comments_on_another_head_hold_the_review_until_left_out_deliberately(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    begin(&t, &mut vcx, "Looks close.");
    assert!(
        vcx.debug_bounds("pr-review-first-blocker").is_some(),
        "opening the review immediately shows the first blocker"
    );
    let approve = vcx.debug_bounds("pr-review-event-1").unwrap().center();
    vcx.simulate_click(approve, Modifiers::default());
    assert!(vcx.debug_bounds("pr-review-submit").is_none());
    t.f.ws
        .update(&mut vcx, |ws, cx| ws.submit_pr_review(t.id, cx));
    vcx.run_until_parked();
    assert_eq!(
        state(&t, &mut vcx),
        Some(SubmitState::Blocked(SubmitBlock::Unanchored(1)))
    );
    assert!(submits(&t.gh).is_empty(), "nothing sent");

    // Pressed twice before the answer: sent once.
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(t.id, cx);
        ws.submit_pr_review_without_unanchored(t.id, cx);
    });
    vcx.run_until_parked();
    assert_eq!(submits(&t.gh), [("h1".to_owned(), ReviewEvent::Approve, 1)]);
    assert_eq!(state(&t, &mut vcx), None, "submitted: the review closes");
    vcx.update(|_, cx| DraftStore::flush(cx));
    let kept = load_review_drafts_in(t.dir.path()).unwrap();
    let left: Vec<&str> = kept.prs[0]
        .comments
        .iter()
        .map(|c| c.body.as_str())
        .collect();
    assert_eq!(left, ["Stale."], "only what was sent leaves the drafts");
    assert_eq!(kept.prs[0].review_body, None);
}

#[gpui::test]
fn individual_blockers_can_be_excluded_or_reattached_before_submitting(cx: &mut TestAppContext) {
    use crate::workspace::pages::pull_requests::detail::diff_layout::Side;
    let t = reviewing(cx);
    let owner = cx.update(|cx| DraftStore::owner(&repo(), 7, cx)).unwrap();
    cx.update(|cx| {
        let mut old = DraftStore::of(&owner, cx).unwrap().comments.pop().unwrap();
        old.body = "Reattach this.".into();
        DraftStore::put_comment(&owner, None, old, cx);
    });
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1600.), px(1000.)));
    begin(&t, &mut vcx, "Review summary");
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_review_event(t.id, ReviewEvent::RequestChanges, cx);
        ws.submit_pr_review(t.id, cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        state(&t, &mut vcx),
        Some(SubmitState::Blocked(SubmitBlock::Unanchored(2)))
    );
    let click = |key: &'static str, vcx: &mut VisualTestContext| {
        let at = vcx.debug_bounds(key).unwrap().center();
        vcx.simulate_click(at, Modifiers::default());
        vcx.run_until_parked();
    };
    click("pr-review-first-blocker", &mut vcx);
    click("pr-composer-exclude", &mut vcx);
    let comment_counts = |vcx: &mut VisualTestContext| {
        use crate::workspace::pages::pull_requests::detail::view::DetailBody;
        t.f.ws.update(vcx, |ws, cx| {
            ws.stage_pr_page(cx);
            let snapshot = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
            let DetailBody::Ready(ready) = &snapshot.body else {
                panic!("read detail")
            };
            let review = ready
                .review_comments
                .as_ref()
                .unwrap()
                .review
                .as_ref()
                .unwrap();
            (review.included_comments, review.excluded_comments)
        })
    };
    assert_eq!(comment_counts(&mut vcx), (1, 1));
    click("pr-review-first-blocker", &mut vcx);
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.select_pr_line(t.id, Side::New, 2, false, cx)
    });
    vcx.run_until_parked();
    click("pr-composer-reanchor", &mut vcx);
    assert!(vcx.debug_bounds("pr-review-first-blocker").is_none());
    assert_eq!(comment_counts(&mut vcx), (2, 1));
    assert!(submits(&t.gh).is_empty());
    click("pr-review-submit", &mut vcx);
    assert_eq!(
        submits(&t.gh),
        [("h1".into(), ReviewEvent::RequestChanges, 2)]
    );
    let left = vcx.update(|_, cx| DraftStore::of(&owner, cx).unwrap().comments);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].body, "Stale.");
}

#[gpui::test]
fn a_review_whose_answer_was_lost_is_found_taken_and_not_offered_again(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    begin(&t, &mut vcx, "Ship it once the nits are in.");
    t.gh.fail(
        GhOp::SubmitReview,
        GhFailure::Exit {
            code: None,
            stderr: "timed out".into(),
        },
    );
    t.gh.set_review_lands_anyway(true);
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(t.id, cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        state(&t, &mut vcx),
        None,
        "the latest review is this one: taken"
    );
    assert_eq!(submits(&t.gh).len(), 1);

    // Failing for real keeps everything to submit again.
    t.gh.set_review_lands_anyway(false);
    begin(&t, &mut vcx, "Second look.");
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(t.id, cx)
    });
    vcx.run_until_parked();
    assert!(matches!(state(&t, &mut vcx), Some(SubmitState::Failed(_))));
    vcx.update(|_, cx| DraftStore::flush(cx));
    let kept = load_review_drafts_in(t.dir.path()).unwrap();
    assert_eq!(kept.prs[0].review_body.as_deref(), Some("Second look."));
}

/// Sent while a refresh is out, the review goes at the head shown, not
/// one GitHub has moved to since.
#[gpui::test]
fn a_review_sent_while_a_refresh_is_out_goes_at_the_head_shown(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    begin(&t, &mut vcx, "Ship it.");
    let mut moved = mergeable(7, "feat");
    moved.head_sha = "h2".into();
    moved.author = Some("alice".into());
    t.gh.set_pr_detail(&repo(), 7, moved);
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.refresh_pr_detail(t.id, cx);
        ws.submit_pr_review_without_unanchored(t.id, cx);
    });
    vcx.run_until_parked();
    let commits: Vec<String> = submits(&t.gh)
        .into_iter()
        .map(|(commit, ..)| commit)
        .collect();
    assert_eq!(commits, ["h1"]);
}

#[gpui::test]
fn an_author_cannot_approve_their_own_pull_request(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut detail = mergeable(7, "feat");
    detail.head_sha = "h1".into();
    detail.author = Some("ME".into());
    t.gh.set_pr_detail(&repo(), 7, detail);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    t.f.ws
        .update(&mut vcx, |ws, cx| ws.refresh_pr_detail(t.id, cx));
    vcx.run_until_parked();
    begin(&t, &mut vcx, "LGTM");
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.set_pr_review_event(t.id, ReviewEvent::Approve, cx);
        ws.submit_pr_review_without_unanchored(t.id, cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        state(&t, &mut vcx),
        Some(SubmitState::Blocked(SubmitBlock::OwnPullRequest))
    );
    assert!(submits(&t.gh).is_empty());
}

#[gpui::test]
fn an_outcome_that_cannot_be_read_is_checked_never_sent_again(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    begin(&t, &mut vcx, "Ship it.");
    // GitHub cannot be read, and the submission's answer is lost though it
    // landed.
    let down = || GhFailure::Exit {
        code: None,
        stderr: "timed out".into(),
    };
    t.gh.fail(GhOp::LatestReview, down());
    t.gh.fail(GhOp::SubmitReview, down());
    t.gh.set_review_lands_anyway(true);
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(t.id, cx)
    });
    vcx.run_until_parked();
    assert!(matches!(
        state(&t, &mut vcx),
        Some(SubmitState::Unknown { .. })
    ));
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(t.id, cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        submits(&t.gh).len(),
        1,
        "no second submission while unknown"
    );

    // Leaving and reopening does not open a way around it either.
    vcx.update(|window, cx| t.f.ws.update(cx, |ws, cx| ws.back_to_pr_list(window, cx)));
    open(&t.f, item(7, "feat"), &mut vcx);
    vcx.run_until_parked();
    let again =
        t.f.ws
            .read_with(&vcx, |ws, _| ws.pages.pull_requests.detail().unwrap().id());
    let reopened = Fixture { id: again, ..t };
    begin(&reopened, &mut vcx, "Ship it.");
    reopened.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(again, cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        submits(&reopened.gh).len(),
        1,
        "still unsettled in the whole app"
    );
    assert!(
        matches!(
            state(&reopened, &mut vcx),
            Some(SubmitState::Unknown { .. })
        ),
        "said, with a check to run"
    );
    reopened.gh.clear_failure(GhOp::LatestReview);
    reopened
        .f
        .ws
        .update(&mut vcx, |ws, cx| ws.check_pr_review(again, cx));
    vcx.run_until_parked();
    assert_eq!(state(&reopened, &mut vcx), None, "the review had landed");
    assert_eq!(submits(&reopened.gh).len(), 1);
}

#[gpui::test]
fn checking_again_finds_a_review_that_landed(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    begin(&t, &mut vcx, "Ship it.");
    let down = || GhFailure::Exit {
        code: None,
        stderr: "timed out".into(),
    };
    t.gh.fail(GhOp::LatestReview, down());
    t.gh.fail(GhOp::SubmitReview, down());
    t.gh.set_review_lands_anyway(true);
    t.f.ws.update(&mut vcx, |ws, cx| {
        ws.submit_pr_review_without_unanchored(t.id, cx)
    });
    vcx.run_until_parked();
    t.gh.clear_failure(GhOp::LatestReview);
    let check = vcx.debug_bounds("pr-review-check").unwrap().center();
    vcx.simulate_click(check, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(state(&t, &mut vcx), None, "found taken: done");
    assert_eq!(submits(&t.gh).len(), 1);
}

/// Signed in as another account since the review opened: it sends that
/// account's drafts, the login `gh` submits as, never the earlier one's.
#[gpui::test]
fn a_review_follows_the_account_signed_in_when_sent(cx: &mut TestAppContext) {
    let t = reviewing(cx);
    let mut vcx = VisualTestContext::from_window(t.f.window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    begin(&t, &mut vcx, "Looks good.");
    t.gh.set_account(
        "github.com",
        GhAccount {
            login: "you".into(),
            source: CredentialSource::Env("GH_TOKEN".into()),
        },
    );
    t.f.ws
        .update(&mut vcx, |ws, cx| ws.recheck_gh("github.com", cx));
    vcx.update(|_, cx| crate::github::PrPoller::tick(cx));
    vcx.run_until_parked();
    t.f.ws
        .update(&mut vcx, |ws, cx| ws.submit_pr_review(t.id, cx));
    vcx.run_until_parked();
    assert_eq!(
        submits(&t.gh),
        vec![("h1".to_owned(), ReviewEvent::Comment, 0)],
        "`me`'s comments stay `me`'s drafts"
    );
    let owner = vcx
        .update(|_, cx| DraftStore::owner(&repo(), 7, cx))
        .unwrap();
    assert_eq!(owner.account, "you");
}
