//! Creating a pull request stacked on another: where it would sit is
//! settled before anything is pushed, it is stacked once it exists, and a
//! stacking that fails leaves it to be stacked again from the form.

use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{CheckRollup, PrStack, PrState, PrSummary, RepoId, StackEntry, StackPlace};
use daruda_github::{Gh as _, StackRead};
use daruda_store::project::LaneRef;
use gpui::{AppContext as _, Entity, TestAppContext};

use super::GitFixture;
use super::github_link::{lane_with, pushes, request, toasts};
use crate::workspace::github_ops::create_modal::CreatePrModal;

const PARENT: u64 = 5;
/// The number the fake gives the pull request created after `PARENT`.
const CREATED: u64 = 6;

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

/// GitHub with pull request `PARENT` open on the branch `parent`.
fn gh_with_parent(cx: &mut TestAppContext) -> FakeGh {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(
        &repo(),
        vec![PrSummary {
            number: PARENT,
            url: format!("https://github.com/o/r/pull/{PARENT}"),
            title: "The parent".into(),
            state: PrState::Open,
            head_branch: "parent".into(),
            cross_repo: false,
            checks: CheckRollup::Passing,
            failed: Vec::new(),
            review: None,
            updated_at: "2026-01-01".into(),
        }],
    );
    gh
}

fn stack(number: u64, members: &[u64]) -> StackRead {
    StackRead::Stacked(PrStack {
        number,
        base: "main".into(),
        size: members.len() as u32,
        entries: members
            .iter()
            .zip(1..)
            .map(|(&number, position)| StackEntry {
                position,
                number,
                title: String::new(),
                state: PrState::Open,
            })
            .collect(),
    })
}

/// Submit a form for `lane` asking to stack on `PARENT`.
fn create_stacked(f: &GitFixture, lane: LaneRef, cx: &mut TestAppContext) -> Entity<CreatePrModal> {
    let window: gpui::AnyWindowHandle = f.window.into();
    let ws = f.ws.downgrade();
    let root = f.root.clone();
    let modal = cx
        .update_window(window, |_, window, cx| {
            let repo = crate::git_host::GitHost::global(cx).open(&root);
            cx.new(|cx| CreatePrModal::new_for_test(ws, lane, repo, window, cx))
        })
        .unwrap();
    let weak = modal.downgrade();
    let mut asked = request(Some("parent"));
    asked.stack_on = Some(PARENT);
    f.ws.update(cx, |ws, cx| ws.create_pr(lane, asked, weak, window, cx));
    cx.run_until_parked();
    modal
}

fn stack_calls(gh: &FakeGh) -> Vec<GhCall> {
    gh.calls()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                GhCall::PrStack { .. } | GhCall::CreatePr { .. } | GhCall::StackPr { .. }
            )
        })
        .collect()
}

/// The failures of stacking told as toasts.
fn stacking_toasts(f: &GitFixture, cx: &mut TestAppContext) -> Vec<String> {
    let title = crate::surface::strings::error::github_pr_stack_failed();
    toasts(f, cx)
        .into_iter()
        .filter(|toast| toast.contains(&title))
        .collect()
}

fn members(gh: &FakeGh, number: u64) -> Vec<u64> {
    match gh.pr_stack(&repo(), number).unwrap() {
        StackRead::Stacked(stack) => stack.entries.iter().map(|entry| entry.number).collect(),
        StackRead::NotStacked | StackRead::Unsupported => Vec::new(),
    }
}

/// The parent's stack is read, the pull request created against the
/// parent's branch, then put where the read said: with a parent in no
/// stack the two make a new one; on top of a parent that is the top.
#[gpui::test]
fn a_stacked_pull_request_is_created_then_put_on_its_parent(cx: &mut TestAppContext) {
    let cases = [
        (
            "a parent in no stack",
            None,
            StackPlace::New { parent: PARENT },
            vec![PARENT, CREATED],
        ),
        (
            "a parent at the top of a stack",
            Some(stack(3, &[2, PARENT])),
            StackPlace::Top { stack: 3 },
            vec![2, PARENT, CREATED],
        ),
    ];
    for (label, parents_stack, place, expected) in cases {
        let gh = gh_with_parent(cx);
        if let Some(read) = parents_stack {
            gh.set_pr_stack(&repo(), PARENT, read);
        }
        let (f, id) = lane_with(cx, false);
        let modal = create_stacked(&f, id, cx);
        modal.read_with(cx, |modal, _| {
            assert_eq!(modal.failure(), None, "{label}");
            assert!(modal.unstacked().is_none(), "{label}");
        });
        let calls = stack_calls(&gh);
        assert!(
            matches!(
                calls.as_slice(),
                [
                    GhCall::PrStack { number: PARENT, .. },
                    GhCall::CreatePr { new, .. },
                    GhCall::StackPr { place: put, number: CREATED, .. },
                ] if new.base.as_deref() == Some("parent") && *put == place
            ),
            "{label}: {calls:?}"
        );
        assert_eq!(members(&gh, CREATED), expected, "{label}");
    }
}

/// Only the top of a stack takes another: with one above the parent, or
/// on a server without stacks, nothing is pushed and nothing created.
#[gpui::test]
fn a_parent_that_takes_none_stops_before_the_push(cx: &mut TestAppContext) {
    use crate::surface::strings::{error as es, git as s};
    let cases = [
        (
            "one already above the parent",
            stack(3, &[PARENT, 9]),
            es::github_pr_stack_not_top(s::pr_number(PARENT)),
        ),
        (
            "a server without stacks",
            StackRead::Unsupported,
            es::github_pr_stack_unsupported(),
        ),
    ];
    for (label, read, said) in cases {
        let gh = gh_with_parent(cx);
        gh.set_pr_stack(&repo(), PARENT, read);
        let (f, id) = lane_with(cx, false);
        let modal = create_stacked(&f, id, cx);
        assert_eq!(
            modal.read_with(cx, |modal, _| modal.failure().map(str::to_owned)),
            Some(said),
            "{label}"
        );
        assert!(pushes(&f).is_empty(), "{label}");
        assert_eq!(gh.count(GhOp::CreatePr), 0, "{label}");
    }
}

/// The pull request exists once created: a stacking GitHub refuses leaves
/// it, the form says what is left, and stacking again goes through
/// without creating a second one.
#[gpui::test]
fn a_refused_stacking_keeps_the_pull_request_and_is_tried_again(cx: &mut TestAppContext) {
    let gh = gh_with_parent(cx);
    gh.fail(
        GhOp::StackPr,
        GhFailure::Exit {
            code: Some(1),
            stderr: "Validation Failed (HTTP 422)".into(),
        },
    );
    let (f, id) = lane_with(cx, false);
    let modal = create_stacked(&f, id, cx);
    assert_eq!(gh.count(GhOp::CreatePr), 1);
    modal.read_with(cx, |modal, _| {
        let (left, retrying) = modal.unstacked().expect("created, not stacked");
        assert_eq!((left.parent, left.number), (PARENT, Some(CREATED)));
        assert!(left.reason.contains("Validation Failed"), "{}", left.reason);
        assert!(!retrying);
    });
    assert!(members(&gh, CREATED).is_empty());

    // Published: typing in the form left open keeps nothing for the
    // branch, and nothing is written or created from it.
    cx.update_window(f.window.into(), |_, window, cx| {
        modal.update(cx, |modal, cx| {
            modal.type_for_test("Typed after", "", window, cx);
            assert!(modal.is_created());
        });
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::CreatePr), 1);

    // Still refused: the form says why again, and stays.
    let again = |cx: &mut TestAppContext| {
        cx.update_window(f.window.into(), |_, window, cx| {
            modal.update(cx, |modal, cx| modal.stack_again_for_test(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
    };
    again(cx);
    assert!(modal.read_with(cx, |modal, _| modal.unstacked().is_some_and(|(_, r)| !r)));

    gh.clear_failure(GhOp::StackPr);
    again(cx);
    assert_eq!(members(&gh, CREATED), [PARENT, CREATED]);
    assert_eq!(gh.count(GhOp::CreatePr), 1, "not created twice");
}

/// An attempt whose answer was lost may have gone through: stacking again
/// reads the stack first and sends nothing when the pull request is in it.
#[gpui::test]
fn stacking_again_sends_nothing_for_one_already_stacked(cx: &mut TestAppContext) {
    let gh = gh_with_parent(cx);
    gh.fail(GhOp::StackPr, GhFailure::Timeout);
    let (f, id) = lane_with(cx, false);
    let modal = create_stacked(&f, id, cx);
    let reason = |cx: &mut TestAppContext| {
        modal.read_with(cx, |modal, _| {
            modal.unstacked().map(|(left, _)| left.reason.clone())
        })
    };
    let first = reason(cx).expect("created, not stacked");
    // It went through after all.
    gh.clear_failure(GhOp::StackPr);
    gh.set_pr_stack(&repo(), PARENT, stack(3, &[PARENT, CREATED]));
    let sent = gh.count(GhOp::StackPr);
    cx.update_window(f.window.into(), |_, window, cx| {
        modal.update(cx, |modal, cx| modal.stack_again_for_test(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::StackPr), sent, "nothing sent");
    // The parent is no longer the top, but that is not reported: the one
    // above it is this pull request.
    assert_eq!(reason(cx), Some(first), "no new failure");
    assert_eq!(stacking_toasts(&f, cx), [""; 0]);
    // Taken as stacked: the form waits only for its dialog to close, and
    // does not offer stacking again.
    assert!(modal.read_with(cx, |modal, _| modal.unstacked().is_some_and(|(_, r)| r)));
}

/// The form closed while stacking again was in flight: the failure is
/// still told, by the workspace, and the request was still sent.
#[gpui::test]
fn stacking_again_outlives_its_form(cx: &mut TestAppContext) {
    let gh = gh_with_parent(cx);
    gh.fail(
        GhOp::StackPr,
        GhFailure::Exit {
            code: Some(1),
            stderr: "Validation Failed".into(),
        },
    );
    let (f, id) = lane_with(cx, false);
    let modal = create_stacked(&f, id, cx);
    assert_eq!(stacking_toasts(&f, cx), [""; 0], "not told yet");
    let sent = gh.count(GhOp::StackPr);
    cx.update_window(f.window.into(), |_, window, cx| {
        modal.update(cx, |modal, cx| modal.stack_again_for_test(window, cx));
    })
    .unwrap();
    drop(modal);
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::StackPr), sent + 1);
    let told = stacking_toasts(&f, cx);
    assert!(
        told.iter().any(|toast| toast.contains("Validation Failed")),
        "{told:?}"
    );
}

/// The branch already has a pull request (an earlier create whose answer
/// was lost): it is not created again, and with stacking asked for the
/// form says it is not in the stack, unless it is.
#[gpui::test]
fn an_existing_pull_request_is_not_passed_off_as_stacked(cx: &mut TestAppContext) {
    const EXISTING: u64 = 7;
    let cases = [
        ("in no stack", None, true),
        (
            "in the parent's stack",
            Some(stack(3, &[PARENT, EXISTING])),
            false,
        ),
    ];
    for (label, parents_stack, owed) in cases {
        let gh = gh_with_parent(cx);
        let mut prs = gh
            .prs_for_branches(&repo(), &["parent".into()], &[])
            .unwrap();
        let mut all = prs.by_branch.remove("parent").unwrap();
        let mut existing = all[0].clone();
        existing.number = EXISTING;
        existing.url = format!("https://github.com/o/r/pull/{EXISTING}");
        existing.head_branch = "feat".into();
        all.push(existing);
        gh.set_prs(&repo(), all);
        if let Some(read) = parents_stack {
            gh.set_pr_stack(&repo(), PARENT, read);
        }
        let (f, id) = lane_with(cx, true);
        let modal = create_stacked(&f, id, cx);
        assert_eq!(
            (
                gh.count(GhOp::CreatePr),
                gh.count(GhOp::StackPr),
                pushes(&f).len()
            ),
            (0, 0, 0),
            "{label}: nothing pushed, created or stacked"
        );
        modal.read_with(cx, |modal, _| {
            let left = modal
                .unstacked()
                .map(|(left, _)| (left.parent, left.number));
            if owed {
                assert_eq!(left, Some((PARENT, Some(EXISTING))), "{label}");
                assert!(modal.existing().is_none(), "{label}");
            } else {
                assert_eq!(left, None, "{label}");
                assert_eq!(
                    modal.existing().map(|created| created.number),
                    Some(Some(EXISTING)),
                    "{label}"
                );
            }
        });
    }
}
