//! Asking for reviews from the detail's side column.

use daruda_github::ReviewerState;
use daruda_github::fake::GhCall;
use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

use super::pull_request_detail::{mergeable, opened};
use crate::workspace::pages::pull_requests::detail::reviewers::ReviewerOp;

fn type_reviewers(f: &super::GitFixture, vcx: &mut VisualTestContext, text: &str) {
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let detail = ws.pages.pull_requests.detail().unwrap();
            let input = detail.reviewer_requests().input.clone().unwrap();
            input.update(cx, |input, cx| input.set_value(text.to_owned(), window, cx));
        })
    });
}

fn reviewers(f: &super::GitFixture, vcx: &mut VisualTestContext) -> Vec<(String, ReviewerState)> {
    f.ws.read_with(vcx, |ws, _| {
        let detail = ws.pages.pull_requests.detail().unwrap();
        match detail.load() {
            crate::workspace::pages::pull_requests::detail::Load::Ready(read) => {
                read.reviewers.clone()
            }
            _ => Vec::new(),
        }
    })
}

#[gpui::test]
fn reviewers_are_asked_and_unasked_and_bad_names_never_reach_gh(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.reviewers = vec![
        ("bob".into(), ReviewerState::Requested),
        ("carol".into(), ReviewerState::Approved),
    ];
    let (f, gh, _) = opened(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1600.), px(900.)));
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("pr-reviewer-remove-0").is_some(),
        "bob can be unasked"
    );
    assert!(
        vcx.debug_bounds("pr-reviewer-remove-1").is_none(),
        "carol reviewed already"
    );

    let remove = vcx.debug_bounds("pr-reviewer-remove-0").unwrap().center();
    vcx.simulate_click(remove, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        reviewers(&f, &mut vcx),
        [("carol".into(), ReviewerState::Approved)]
    );

    let open = vcx.debug_bounds("pr-reviewers-open").unwrap().center();
    vcx.simulate_click(open, Modifiers::default());
    vcx.run_until_parked();
    type_reviewers(&f, &mut vcx, "dave, --admin");
    let request = vcx.debug_bounds("pr-reviewers-request").unwrap().center();
    vcx.simulate_click(request, Modifiers::default());
    vcx.run_until_parked();
    let op = f.ws.read_with(&vcx, |ws, _| {
        ws.pages
            .pull_requests
            .detail()
            .unwrap()
            .reviewer_requests()
            .op
            .clone()
    });
    assert_eq!(op, Some(ReviewerOp::Invalid("--admin".into())));
    let edits = |gh: &daruda_github::fake::FakeGh| -> Vec<(Vec<String>, Vec<String>)> {
        gh.calls()
            .into_iter()
            .filter_map(|call| match call {
                GhCall::EditPrReviewers { add, remove, .. } => Some((add, remove)),
                _ => None,
            })
            .collect()
    };
    assert_eq!(edits(&gh).len(), 1, "only the removal reached gh");

    type_reviewers(&f, &mut vcx, "@dave org/core");
    let request = vcx.debug_bounds("pr-reviewers-request").unwrap().center();
    vcx.simulate_click(request, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        edits(&gh)[1],
        (vec!["dave".to_owned(), "org/core".to_owned()], Vec::new())
    );
    let names: Vec<String> = reviewers(&f, &mut vcx)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        names,
        ["carol", "dave", "org/core"],
        "read again after the change"
    );
    assert!(
        vcx.debug_bounds("pr-reviewers-request").is_none(),
        "the box closed"
    );
}

#[gpui::test]
fn reviewers_cannot_be_changed_without_permission(cx: &mut TestAppContext) {
    let mut detail = mergeable(7, "feat");
    detail.reviewers = vec![("bob".into(), ReviewerState::Requested)];
    let (f, _, _) = opened(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1600.), px(900.)));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("pr-reviewers-open").is_none());
    assert!(vcx.debug_bounds("pr-reviewer-remove-0").is_none());
}

#[gpui::test]
fn the_author_is_not_asked_and_a_failed_request_keeps_the_text(cx: &mut TestAppContext) {
    use daruda_github::fake::{GhFailure, GhOp};
    let mut detail = mergeable(7, "feat");
    detail.viewer_can = detail
        .viewer_can
        .with(daruda_github::Permission::Update, true);
    detail.author = Some("alice".into());
    let (f, gh, _) = opened(cx, detail);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.simulate_resize(size(px(1600.), px(900.)));
    vcx.run_until_parked();
    let open = vcx.debug_bounds("pr-reviewers-open").unwrap().center();
    vcx.simulate_click(open, Modifiers::default());
    vcx.run_until_parked();
    let state = |f: &super::GitFixture, vcx: &mut VisualTestContext| {
        vcx.update(|_, cx| {
            f.ws.read_with(cx, |ws, cx| {
                let requests = ws.pages.pull_requests.detail().unwrap().reviewer_requests();
                (
                    requests
                        .input
                        .as_ref()
                        .map(|input| input.read(cx).value().to_string()),
                    requests.op.clone(),
                )
            })
        })
    };
    type_reviewers(&f, &mut vcx, "bob Alice");
    let request = vcx.debug_bounds("pr-reviewers-request").unwrap().center();
    vcx.simulate_click(request, Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        state(&f, &mut vcx).1,
        Some(ReviewerOp::Author("Alice".into()))
    );
    assert!(
        !gh.calls()
            .iter()
            .any(|call| matches!(call, GhCall::EditPrReviewers { .. })),
        "GitHub would refuse the whole request"
    );

    type_reviewers(&f, &mut vcx, "bob");
    assert_eq!(state(&f, &mut vcx).1, None, "typing clears the line");
    gh.fail(
        GhOp::EditPrReviewers,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 422".into(),
        },
    );
    let request = vcx.debug_bounds("pr-reviewers-request").unwrap().center();
    vcx.simulate_click(request, Modifiers::default());
    vcx.run_until_parked();
    let (text, op) = state(&f, &mut vcx);
    assert_eq!(text.as_deref(), Some("bob"), "kept to ask again");
    assert!(matches!(op, Some(ReviewerOp::Failed(reason)) if reason.contains("HTTP 422")));
}
