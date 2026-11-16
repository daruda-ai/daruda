//! A fork pull request's review lane in the Git panel: its mark, not its
//! branch name, finds its pull request, and it is never offered one of its
//! own to create.

use daruda_github::fake::GhCall;
use daruda_github::{CheckRollup, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::TestAppContext;

use super::GitFixture;
use crate::github::{PrLookup, PrPoller};
use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::github_ops::strip::PrStrip;

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

/// Pull request 9 from a fork whose branch is named `main`.
fn fork_pr() -> PrSummary {
    PrSummary {
        number: 9,
        url: "https://github.com/o/r/pull/9".into(),
        title: "From a fork".into(),
        state: PrState::Open,
        head_branch: "main".into(),
        cross_repo: true,
        checks: CheckRollup::Failing { failed: Some(1) },
        failed: Vec::new(),
        review: None,
        updated_at: "2026-01-01".into(),
    }
}

/// A lane on review branch `pr-9`, which has no upstream, so its push
/// target falls back to `origin/pr-9`; read and registered with the poller.
fn review_lane(cx: &mut TestAppContext, review: Option<&str>) -> (GitFixture, LaneRef) {
    let target = PushTarget {
        remote: "origin".into(),
        url: "git@github.com:o/r.git".into(),
        branch: "pr-9".into(),
        published: false,
        has_upstream: false,
        review: review.map(str::to_owned),
    };
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(target)
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    (f, id)
}

fn tick(cx: &mut TestAppContext) {
    cx.update(PrPoller::tick);
    cx.run_until_parked();
}

fn lookup(f: &GitFixture, id: LaneRef, cx: &mut TestAppContext) -> PrLookup {
    f.ws.read_with(cx, |ws, _| {
        ws.lane_scoped
            .get(&id)
            .map(|state| state.pr.lookup().clone())
            .unwrap_or_default()
    })
}

#[gpui::test]
fn a_review_lane_is_linked_by_its_mark_not_its_branch(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![fork_pr()]);
    let (f, id) = review_lane(cx, Some("github.com/o/r#9"));
    tick(cx);
    let polls: Vec<GhCall> = gh
        .calls()
        .into_iter()
        .filter(|call| matches!(call, GhCall::PrsForBranches { .. }))
        .collect();
    assert_eq!(
        polls,
        vec![GhCall::PrsForBranches {
            repo: repo(),
            branches: vec![],
            numbers: vec![9],
        }],
        "asked by number; `pr-9` is no branch of the base repository"
    );
    let strip = f.ws.read_with(cx, |ws, cx| ws.pr_strip(cx));
    let PrStrip::Linked(link) = strip else {
        panic!("expected the review lane linked, got {strip:?}");
    };
    assert_eq!((&link.repo, link.pr.number), (&repo(), 9));
    assert_eq!(
        f.ws.read_with(cx, |ws, _| ws.open_lane_pr_count()),
        1,
        "counted like any linked lane"
    );
    assert_eq!(lookup(&f, id, cx), PrLookup::Linked(Box::new(link)));
}

#[gpui::test]
fn a_review_lane_is_never_offered_a_pull_request_to_create(cx: &mut TestAppContext) {
    // (label, review mark)
    let cases = [
        ("GitHub has no such pull request", "github.com/o/r#9"),
        ("a mark daruda cannot read", "not a mark"),
    ];
    for (label, mark) in cases {
        let _gh = crate::test_support::fake_gh(cx);
        let (f, id) = review_lane(cx, Some(mark));
        tick(cx);
        assert_eq!(lookup(&f, id, cx), PrLookup::Unknown, "{label}");
        let strip = f.ws.read_with(cx, |ws, cx| ws.pr_strip(cx));
        assert_eq!(strip, PrStrip::Hidden, "{label}");
    }
}

#[gpui::test]
fn the_same_branch_without_a_mark_is_an_ordinary_branch(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![fork_pr()]);
    let (f, _id) = review_lane(cx, None);
    tick(cx);
    let strip = f.ws.read_with(cx, |ws, cx| ws.pr_strip(cx));
    assert_eq!(strip, PrStrip::NoPr, "a user's own `pr-9` may open its PR");
}

#[gpui::test]
fn a_review_lanes_missing_pr_pauses_no_other_lane(cx: &mut TestAppContext) {
    let gh = crate::test_support::fake_gh(cx);
    let mut feat = fork_pr();
    feat.number = 7;
    feat.head_branch = "feat".into();
    feat.cross_repo = false;
    gh.set_prs(&repo(), vec![feat]);
    let (review, review_id) = review_lane(cx, Some("github.com/o/r#9"));
    let target = PushTarget {
        remote: "origin".into(),
        url: "git@github.com:o/r.git".into(),
        branch: "feat".into(),
        published: true,
        has_upstream: false,
        review: None,
    };
    let other = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(target)
    });
    let other_id = other.ws.read_with(cx, |ws, _| ws.active_ref());
    other
        .ws
        .update(cx, |ws, cx| ws.refresh_git_status(other_id, cx));
    cx.run_until_parked();
    let (window, ws) = (other.window, other.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    tick(cx);

    assert_eq!(lookup(&review, review_id, cx), PrLookup::Unknown);
    let linked = lookup(&other, other_id, cx);
    assert_eq!(linked.link().map(|link| link.pr.number), Some(7));
    let polls: Vec<GhCall> = gh
        .calls()
        .into_iter()
        .filter(|call| matches!(call, GhCall::PrsForBranches { .. }))
        .collect();
    assert_eq!(
        polls.last(),
        Some(&GhCall::PrsForBranches {
            repo: repo(),
            branches: vec!["feat".into()],
            numbers: vec![9],
        }),
        "one request answers both lanes"
    );
}
