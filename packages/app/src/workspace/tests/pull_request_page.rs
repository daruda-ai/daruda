//! The Pull requests page in a window: what opening, filtering and closing
//! it searches, and where its rows lead. What a list comes to for given
//! inputs is tested beside `pages/pull_requests/list.rs`.

use std::time::Duration;

use daruda_github::fake::{FakeGh, GhCall, GhFailure, GhOp};
use daruda_github::{CheckRollup, PrFilter, PrListItem, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::{Modifiers, TestAppContext, VisualTestContext};

use crate::github::PrPoller;
use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::list::{GroupBody, PageBody, RowLaneSlot, Trouble};
use crate::workspace::tests::GitFixture;

fn repo() -> RepoId {
    RepoId::new("github.com", "o", "r")
}

fn summary(number: u64, head: &str) -> PrSummary {
    PrSummary {
        number,
        url: format!("https://github.com/o/r/pull/{number}"),
        title: format!("PR {number}"),
        state: PrState::Open,
        head_branch: head.into(),
        cross_repo: false,
        checks: CheckRollup::Passing,
        failed: Vec::new(),
        review: None,
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

fn item(number: u64) -> PrListItem {
    PrListItem {
        repo: repo(),
        pr: summary(number, &format!("head-{number}")),
        author: Some("me".into()),
        head_owner: None,
        base_branch: "main".into(),
        comments: 0,
    }
}

fn tick(cx: &mut TestAppContext) {
    cx.update(PrPoller::tick);
    cx.run_until_parked();
}

/// A window whose one lane pushes `feat` to o/r, linked to PR #7, with
/// #7 and #8 among the user's open pull requests.
fn linked_window(cx: &mut TestAppContext) -> (GitFixture, FakeGh, LaneRef) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![summary(7, "feat")]);
    gh.set_pr_list(PrFilter::Mine, vec![item(7), item(8)]);
    gh.set_pr_list(PrFilter::ReviewRequested, vec![item(8)]);
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: false,
        })
    });
    let id = f.ws.read_with(cx, |ws, _| ws.active_ref());
    f.ws.update(cx, |ws, cx| ws.refresh_git_status(id, cx));
    cx.run_until_parked();
    let (window, ws) = (f.window, f.ws.downgrade());
    cx.update(|cx| crate::window_registry::WindowRegistry::register(window.into(), ws, cx));
    tick(cx);
    (f, gh, id)
}

fn show_page(f: &GitFixture, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.run_until_parked();
}

fn searches(gh: &FakeGh) -> Vec<PrFilter> {
    gh.calls()
        .into_iter()
        .filter_map(|call| match call {
            GhCall::SearchPrs { filter, .. } => Some(filter),
            _ => None,
        })
        .collect()
}

fn linked(slot: &RowLaneSlot) -> Option<LaneRef> {
    match slot {
        RowLaneSlot::Linked(lane) => Some(lane.lane),
        RowLaneSlot::Creating | RowLaneSlot::Absent => None,
    }
}

fn rows(f: &GitFixture, cx: &mut TestAppContext) -> Vec<(String, Option<LaneRef>)> {
    let list = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = list.body else {
        panic!("expected groups, got {:?}", list.body);
    };
    groups
        .iter()
        .flat_map(|group| {
            let GroupBody::Listed { rows, .. } = &group.body else {
                panic!("{}: {:?}", group.label, group.body);
            };
            rows.iter()
                .map(|row| (row.key.clone(), linked(&row.lane)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Why the page's one group may be out of date.
fn trouble(f: &GitFixture, cx: &mut TestAppContext) -> Option<Trouble> {
    let list = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx));
    let PageBody::Groups(groups) = list.body else {
        panic!("expected groups, got {:?}", list.body);
    };
    match &groups[0].body {
        GroupBody::Listed { trouble, .. } => *trouble,
        other => panic!("{other:?}"),
    }
}

#[gpui::test]
fn opening_the_page_lists_its_pull_requests_with_their_lanes(cx: &mut TestAppContext) {
    let (f, gh, id) = linked_window(cx);
    assert!(
        searches(&gh).is_empty(),
        "nothing is searched before the page opens"
    );
    show_page(&f, cx);
    assert_eq!(searches(&gh), vec![PrFilter::Mine]);
    assert_eq!(
        rows(&f, cx),
        vec![
            ("github.com/o/r#7".to_owned(), Some(id)),
            ("github.com/o/r#8".to_owned(), None)
        ]
    );
}

#[gpui::test]
fn clicking_a_rows_lane_goes_to_it(cx: &mut TestAppContext) {
    let (f, _gh, linked) = linked_window(cx);
    // Stand on another lane, so the click has somewhere to go.
    let other_dir = f.root.parent().unwrap().join("other");
    std::fs::create_dir_all(&other_dir).unwrap();
    let other = LaneRef {
        project: linked.project,
        lane: 77,
    };
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| {
        f.ws.update(cx, |ws, cx| {
            let mut lane = crate::lane::Lane::default_for_project(77, other_dir.clone());
            lane.name = Some("other".into());
            ws.project_for_mut(linked.project).unwrap().lanes.push(lane);
            ws.activate_lane(other, window, cx);
            ws.open_page(Page::PullRequests, window, cx);
        });
        window.refresh();
    });
    vcx.run_until_parked();
    assert_eq!(f.ws.read_with(&vcx, |ws, _| ws.active), other);

    let lane = vcx
        .debug_bounds("pr-lane-github.com/o/r#7")
        .expect("the linked row shows its lane");
    vcx.simulate_click(lane.center(), Modifiers::default());
    vcx.run_until_parked();
    f.ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.active, linked);
        assert_eq!(ws.active_page(), None, "the lane is what is shown now");
    });
}

#[gpui::test]
fn a_failed_search_keeps_its_rows_and_try_again_searches_now(cx: &mut TestAppContext) {
    let (f, gh, id) = linked_window(cx);
    show_page(&f, cx);
    gh.fail(
        GhOp::SearchPrs,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    assert_eq!(
        rows(&f, cx).len(),
        2,
        "the last rows stay under the failure"
    );
    assert_eq!(trouble(&f, cx), Some(Trouble::SearchFailed));

    gh.clear_failure(GhOp::SearchPrs);
    gh.set_pr_list(PrFilter::Mine, vec![item(7)]);
    let searched = gh.count(GhOp::SearchPrs);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let retry = vcx
        .debug_bounds("pr-retry-github.com/o/r")
        .expect("the failed group offers Try again");
    vcx.simulate_click(retry.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(gh.count(GhOp::SearchPrs), searched + 1);
    assert_eq!(
        rows(&f, &mut vcx),
        vec![("github.com/o/r#7".to_owned(), Some(id))]
    );
}

#[gpui::test]
fn switching_the_filter_searches_only_that_filter(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    f.ws.update(cx, |ws, cx| {
        ws.set_pr_page_filter(PrFilter::ReviewRequested, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        searches(&gh),
        vec![PrFilter::Mine, PrFilter::ReviewRequested]
    );
    assert_eq!(rows(&f, cx), vec![("github.com/o/r#8".to_owned(), None)]);

    cx.executor().advance_clock(Duration::from_secs(120));
    tick(cx);
    assert_eq!(
        searches(&gh).last(),
        Some(&PrFilter::ReviewRequested),
        "the cadence asks the shown filter"
    );
    assert_eq!(
        searches(&gh)
            .iter()
            .filter(|f| **f == PrFilter::Mine)
            .count(),
        1,
        "the filter left is not asked again"
    );

    // Reopening shows the filter last chosen.
    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    show_page(&f, cx);
    let filter = f.ws.read_with(cx, |ws, _| ws.pr_list().filter());
    assert_eq!(filter, Some(PrFilter::ReviewRequested));
}

#[gpui::test]
fn closing_the_page_stops_its_searches(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    assert_eq!(gh.count(GhOp::SearchPrs), 1);
    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    cx.executor().advance_clock(Duration::from_secs(600));
    tick(cx);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    assert_eq!(
        gh.count(GhOp::SearchPrs),
        1,
        "nothing is searched while closed"
    );
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.pr_list().filter()), None);

    // Another page replacing it stops them too.
    show_page(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::Tasks, cx));
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.pr_list().filter()), None);
}

#[gpui::test]
fn the_saved_state_never_holds_the_page(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    for before in [None, Some(Page::Tasks)] {
        if let Some(page) = before {
            f.ws.update(cx, |ws, cx| ws.show_page(page, cx));
        }
        show_page(&f, cx);
        let saved =
            f.ws.update(cx, |ws, cx| ws.snapshot_for_disk(cx).0.active_page);
        assert_eq!(saved, None, "after {before:?}");
    }
}

#[gpui::test]
fn the_dock_counts_the_open_pull_requests_lanes_are_on(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.open_lane_pr_count()), 1);
}
