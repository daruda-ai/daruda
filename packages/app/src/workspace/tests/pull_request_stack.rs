//! A pull request's native stack beside its detail: shown in order with
//! the one shown marked, its other pull requests opened from there, and
//! nothing from a server without stacks.

use daruda_github::{PrStack, PrState, StackEntry, StackRead};
use gpui::{TestAppContext, VisualTestContext};

use super::pull_request_detail::{item, open, repo, window};
use crate::workspace::pages::pull_requests::detail::view::{DetailBody, StackView};

fn entry(position: u32, number: u64) -> StackEntry {
    StackEntry {
        position,
        number,
        title: format!("PR {number}"),
        state: PrState::Open,
    }
}

fn stack_shown(f: &super::GitFixture, cx: &mut TestAppContext) -> Option<StackView> {
    f.ws.update(cx, |ws, cx| {
        ws.stage_pr_page(cx);
        match &ws.pages.pull_requests.detail_snapshot.as_ref()?.body {
            DetailBody::Ready(ready) => ready.side.stack.clone(),
            _ => None,
        }
    })
}

#[gpui::test]
fn a_stacked_pull_request_shows_its_stack_and_goes_to_the_others(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    gh.set_pr_stack(
        &repo(),
        7,
        StackRead::Stacked(PrStack {
            number: 2,
            base: "main".into(),
            size: 3,
            entries: vec![entry(1, 6), entry(2, 7)],
        }),
    );
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    // Stacked: each refresh reads it again, as its others merge.
    let reads = gh.count(daruda_github::fake::GhOp::PrStack);
    refresh(&f, cx);
    assert_eq!(gh.count(daruda_github::fake::GhOp::PrStack), reads + 1);
    let stack = stack_shown(&f, cx).expect("stacked");
    let order: Vec<(u64, bool)> = stack.entries.iter().map(|e| (e.number, e.shown)).collect();
    assert_eq!(order, [(6, false), (7, true)], "the one shown marked");
    assert_eq!(
        (stack.base.as_str(), stack.more, stack.shown_at),
        ("main", 1, Some(2))
    );

    // Its neighbour opens from there.
    gh.set_pr_detail(
        &repo(),
        6,
        crate::workspace::tests::pull_request_detail::mergeable(6, "base-work"),
    );
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| ws.open_pr_in_daruda(repo(), 6, window, cx));
    });
    cx.run_until_parked();
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws
            .pages
            .pull_requests
            .detail
            .as_ref()
            .map(|d| d.key())),
        Some((repo(), 6))
    );
}

#[gpui::test]
fn a_server_without_stacks_shows_none(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    gh.set_pr_stack(&repo(), 7, StackRead::Unsupported);
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    assert_eq!(stack_shown(&f, cx), None);
    assert_eq!(gh.count(daruda_github::fake::GhOp::PrStack), 1);
    // Answered: not asked again on every refresh.
    refresh(&f, cx);
    assert_eq!(gh.count(daruda_github::fake::GhOp::PrStack), 1);
}

fn refresh(f: &super::GitFixture, cx: &mut TestAppContext) {
    let id = super::pull_request_detail::id(f, cx);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_detail(id, cx));
    cx.run_until_parked();
}

/// Not among the entries listed: no position is claimed.
#[gpui::test]
fn a_pull_request_past_the_listed_entries_claims_no_position(cx: &mut TestAppContext) {
    let (f, gh, _) = window(cx);
    gh.set_pr_stack(
        &repo(),
        7,
        StackRead::Stacked(PrStack {
            number: 2,
            base: "main".into(),
            size: 30,
            entries: vec![entry(1, 6)],
        }),
    );
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let stack = stack_shown(&f, cx).expect("stacked");
    assert_eq!((stack.shown_at, stack.more), (None, 29));
}
