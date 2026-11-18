//! The Pull requests page in a window: what opening, filtering and closing
//! it searches, and where its rows lead. What a list comes to for given
//! inputs is tested beside `pages/pull_requests/list.rs`.

use std::time::Duration;

use daruda_github::fake::{FakeGh, GhFailure, GhOp};
use daruda_github::{CheckRollup, PrAudience, PrListItem, PrState, PrSummary, RepoId};
use daruda_store::project::LaneRef;
use gpui::{Modifiers, TestAppContext, VisualTestContext};

use crate::github::PrPoller;
use crate::lane::git::{FakeRepo, PushTarget};
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::list::{GroupBody, LaneSlot, PageBody, Trouble};
use crate::workspace::tests::GitFixture;

pub(super) fn repo() -> RepoId {
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

pub(in crate::workspace) fn item(number: u64) -> PrListItem {
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
pub(in crate::workspace) fn linked_window(
    cx: &mut TestAppContext,
) -> (GitFixture, FakeGh, LaneRef) {
    let gh = crate::test_support::fake_gh(cx);
    gh.set_prs(&repo(), vec![summary(7, "feat")]);
    gh.set_pr_list(PrAudience::Mine, vec![item(7), item(8)]);
    gh.set_pr_list(PrAudience::ReviewRequested, vec![item(8)]);
    let f = GitFixture::open(cx, &daruda_config::Config::default(), |repo: FakeRepo| {
        repo.push_target(PushTarget {
            remote: "origin".into(),
            url: "git@github.com:o/r.git".into(),
            branch: "feat".into(),
            published: true,
            has_upstream: false,
            review: None,
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

pub(in crate::workspace) fn show_page(f: &GitFixture, cx: &mut TestAppContext) {
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    cx.run_until_parked();
}

fn searches(gh: &FakeGh) -> Vec<PrAudience> {
    gh.calls()
        .into_iter()
        .filter_map(|call| crate::test_support::list_search(&call).map(|(filter, _)| filter))
        .collect()
}

fn search_totals(gh: &FakeGh) -> [usize; 3] {
    let searches = searches(gh);
    crate::workspace::pages::pull_requests::list::FILTERS
        .map(|filter| searches.iter().filter(|held| **held == filter).count())
}

fn linked(slot: &LaneSlot) -> Option<LaneRef> {
    match slot {
        LaneSlot::Linked(lane) => Some(lane.lane),
        LaneSlot::Creating { .. } | LaneSlot::Absent { .. } => None,
    }
}

pub(in crate::workspace) fn rows(
    f: &GitFixture,
    cx: &mut TestAppContext,
) -> Vec<(String, Option<LaneRef>)> {
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
pub(super) fn trouble(f: &GitFixture, cx: &mut TestAppContext) -> Option<Trouble> {
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
    assert_eq!(search_totals(&gh), [1, 1, 1]);
    assert_eq!(
        rows(&f, cx),
        vec![
            ("github.com/o/r#7".to_owned(), Some(id)),
            ("github.com/o/r#8".to_owned(), None)
        ]
    );
}

#[gpui::test]
fn pull_request_rows_scroll_vertically_and_fit_narrow_and_wide_sizes(cx: &mut TestAppContext) {
    use gpui::{ScrollDelta, ScrollEvent, ScrollSource, TouchPhase, point, px, size};

    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(PrAudience::Mine, (1..=60).map(item).collect());
    show_page(&f, cx);
    let scroll = f.ws.read_with(cx, |ws, _| {
        ws.workspace_page.state().unwrap().scroll.clone()
    });
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    for width in [800., 1280.] {
        vcx.simulate_resize(size(px(width), px(500.)));
        f.ws.update(&mut vcx, |_, cx| {
            scroll.set_offset(point(px(0.), px(0.)));
            cx.notify();
        });
        vcx.run_until_parked();
        let viewport = scroll.bounds();
        let controls = vcx.debug_bounds("pr-list-controls").unwrap();
        assert!(
            controls.bottom() <= viewport.top(),
            "{width}: controls stay above rows"
        );
        assert_eq!(
            vcx.debug_bounds("workspace-page-scrollbar").unwrap(),
            viewport,
            "{width}: scrollbar stays within the list viewport"
        );
        assert!(
            viewport.bottom() <= px(500.),
            "{width}: viewport={viewport:?}"
        );
        assert!(
            scroll.max_offset().y > px(0.),
            "{width}: rows overflow: {viewport:?}"
        );
        let row = vcx.debug_bounds("pr-row-github.com/o/r#7").unwrap();
        vcx.simulate_event(ScrollEvent {
            source: ScrollSource::Pointer,
            position: viewport.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-200.))),
            touch_phase: TouchPhase::Started,
            modifiers: Default::default(),
        });
        vcx.run_until_parked();
        assert!(scroll.offset().y < px(0.), "{width}: wheel moves the rows");
        assert_eq!(
            vcx.debug_bounds("pr-list-controls").unwrap(),
            controls,
            "{width}: filters and search stay fixed"
        );
        let shifted = vcx.debug_bounds("pr-row-github.com/o/r#7").unwrap();
        assert!(shifted.top() < row.top(), "{width}: rows move on screen");
        assert_eq!(
            shifted.left(),
            row.left(),
            "{width}: vertical scrolling keeps columns in place"
        );
        let compact = vcx.debug_bounds("pr-compact-row-github.com/o/r#7");
        let headings = vcx.debug_bounds("pr-list-headings");
        if width == 800. {
            // Between the docks the columns do not fit: rows stack instead
            // of scrolling sideways, and no headings name absent columns.
            assert!(compact.is_some(), "narrow rows stack their facts");
            assert!(headings.is_none(), "stacked rows have no column headings");
            assert!(
                shifted.right() <= viewport.right(),
                "a stacked row fits the page: {shifted:?} in {viewport:?}"
            );
            assert!(
                vcx.debug_bounds("pr-lane-github.com/o/r#7").is_some(),
                "the worktree stays reachable"
            );
        } else {
            assert!(compact.is_none(), "wide rows keep the table");
            let headings = headings.expect("the table names its columns");
            assert!(
                headings.bottom() <= viewport.top(),
                "column headings stay above the scrolling rows"
            );
        }
        let before_click = scroll.offset().y;
        vcx.simulate_click(
            point(
                viewport.right() - px(crate::ui::theme::SCROLLBAR_MARGIN_R),
                viewport.bottom() - px(crate::ui::theme::CONTROL_TARGET_SIZE),
            ),
            Default::default(),
        );
        vcx.run_until_parked();
        assert!(
            scroll.offset().y < before_click,
            "{width}: scrollbar track moves the list"
        );
    }
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
        GhOp::SearchPrsPage,
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

    gh.clear_failure(GhOp::SearchPrsPage);
    gh.set_pr_list(PrAudience::Mine, vec![item(7)]);
    let searched = search_totals(&gh);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let retry = vcx
        .debug_bounds("pr-retry-github.com/o/r")
        .expect("the failed group offers Try again");
    vcx.simulate_click(retry.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        search_totals(&gh),
        searched.map(|n| n + 1),
        "Try again asks the failed host once per tab"
    );
    assert_eq!(
        rows(&f, &mut vcx),
        vec![("github.com/o/r#7".to_owned(), Some(id))]
    );
}

#[gpui::test]
fn reopening_after_a_failed_search_keeps_manual_refresh_available(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.fail(
        GhOp::SearchPrsPage,
        GhFailure::Exit {
            code: Some(1),
            stderr: "HTTP 502".into(),
        },
    );
    show_page(&f, cx);
    assert_eq!(trouble(&f, cx), Some(Trouble::SearchFailed));
    let attempted = search_totals(&gh);
    assert_eq!(attempted, [1, 1, 1]);

    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    tick(cx);
    gh.clear_failure(GhOp::SearchPrsPage);
    show_page(&f, cx);
    assert_eq!(
        search_totals(&gh),
        attempted,
        "the search backoff is still active"
    );
    f.ws.read_with(cx, |ws, cx| {
        let PageBody::Groups(groups) = ws.pr_page_list(cx).body else {
            panic!("expected repository groups");
        };
        assert!(matches!(groups[0].body, GroupBody::Loading));
    });

    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.run_until_parked();
    let refresh = vcx.debug_bounds("pr-refresh").expect("refresh is visible");
    vcx.simulate_click(refresh.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(
        search_totals(&gh),
        attempted.map(|n| n + 1),
        "a user refresh bypasses the search backoff"
    );
    assert_eq!(rows(&f, &mut vcx).len(), 2);
}

#[gpui::test]
fn switching_the_filter_reuses_answers_and_counts_share_the_cadence(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    f.ws.update(cx, |ws, cx| {
        ws.set_pr_page_filter(PrAudience::ReviewRequested, cx)
    });
    cx.run_until_parked();
    assert_eq!(
        search_totals(&gh),
        [1, 1, 1],
        "switching tabs uses the cache"
    );
    assert_eq!(rows(&f, cx), vec![("github.com/o/r#8".to_owned(), None)]);
    cx.executor().advance_clock(Duration::from_secs(120));
    tick(cx);
    assert_eq!(
        search_totals(&gh),
        [2, 2, 2],
        "rows and counts share the cadence"
    );

    // Reopening shows the filter last chosen.
    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    show_page(&f, cx);
    let filter = f.ws.read_with(cx, |ws, _| ws.pr_page_filter());
    assert_eq!(filter, PrAudience::ReviewRequested);
}

#[gpui::test]
fn refreshing_searches_every_tab_so_counts_do_not_lag(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    let counts = |f: &GitFixture, cx: &mut TestAppContext| {
        f.ws.update(cx, |ws, cx| {
            ws.stage_pr_page(cx);
            ws.pages
                .pull_requests
                .snapshot
                .as_ref()
                .unwrap()
                .counts
                .clone()
        })
    };
    assert_eq!(counts(&f, cx), ["2", "1", "0"]);
    // #8 closed on GitHub: every tab that held it must drop it.
    gh.set_pr_list(PrAudience::Mine, vec![item(7)]);
    gh.set_pr_list(PrAudience::ReviewRequested, Vec::new());
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    assert_eq!(search_totals(&gh), [2, 2, 2]);
    assert_eq!(counts(&f, cx), ["1", "0", "0"]);
}

#[gpui::test]
fn a_worktree_in_a_project_outside_the_scope_still_works_on_its_pull_request(
    cx: &mut TestAppContext,
) {
    use crate::workspace::pages::scope::ProjectScope;
    let (f, _gh, linked) = linked_window(cx);
    // A second copy of o/r, browsed alone: its lanes work on nothing.
    let second = f.ws.update(cx, |ws, _| {
        let push = ws.lane_scoped.get(&linked).unwrap().git.push_target.clone();
        let project = ws.next_project_id;
        ws.next_project_id += 1;
        let other = crate::project::Project::bootstrap_placeholder(
            project,
            std::env::temp_dir().join("daruda-pr-outside-scope"),
        );
        let uuid = other.uuid;
        let lane = LaneRef {
            project,
            lane: other.lanes[0].id,
        };
        ws.projects.push(other);
        ws.lane_scoped_mut(lane).git.push_target = push.map(|mut push| {
            push.branch = "main".into();
            push
        });
        uuid
    });
    f.ws.update(cx, |ws, cx| {
        ws.set_page_scope(ProjectScope::Project(second), cx)
    });
    show_page(&f, cx);
    assert_eq!(
        rows(&f, cx),
        vec![
            ("github.com/o/r#7".to_owned(), Some(linked)),
            ("github.com/o/r#8".to_owned(), None),
        ],
        "the row goes to the worktree working on it, wherever it is"
    );
    let name = f.ws.read_with(cx, |ws, _| {
        ws.pr_detail_lane(&item(7)).map(|lane| lane.name)
    });
    let project = f.ws.read_with(cx, |ws, _| {
        ws.project_for(linked.project).unwrap().name.clone()
    });
    assert!(
        name.as_deref()
            .is_some_and(|name| name.starts_with(&format!("{project} / "))),
        "a lane outside the scope names its project: {name:?}"
    );
}

#[gpui::test]
fn search_ignores_worded_ages_and_blank_input(cx: &mut TestAppContext) {
    let (f, _gh, _) = linked_window(cx);
    show_page(&f, cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    // (typed, rows shown, search kept)
    for (typed, visible, query) in [("ago", 0, "ago"), ("   ", 2, ""), ("HEAD-7", 1, "HEAD-7")] {
        f.ws.update_in(&mut vcx, |ws, window, cx| {
            ws.stage_pr_page(cx);
            let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
            let search = snap.search.clone();
            search.update(cx, |input, cx| {
                input.set_value(typed, window, cx);
            });
        });
        vcx.run_until_parked();
        f.ws.read_with(&vcx, |ws, _| {
            let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
            assert_eq!(snap.visible, visible, "{typed:?}: rows");
            assert_eq!(snap.query, query, "{typed:?}: search");
        });
        assert_eq!(
            vcx.debug_bounds("pr-clear-filters").is_some(),
            !query.is_empty(),
            "{typed:?}: a reset is offered only for a search"
        );
    }
}

#[gpui::test]
fn closing_the_page_stops_its_searches(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    show_page(&f, cx);
    assert_eq!(search_totals(&gh), [1, 1, 1]);
    f.ws.update(cx, |ws, cx| ws.close_page(cx));
    cx.executor().advance_clock(Duration::from_secs(600));
    tick(cx);
    f.ws.update(cx, |ws, cx| ws.refresh_pr_list(cx));
    cx.run_until_parked();
    assert_eq!(
        search_totals(&gh),
        [1, 1, 1],
        "nothing is searched while closed"
    );
    assert!(!f.ws.read_with(cx, |ws, _| ws.pr_list().is_shown()));

    // Another page replacing it stops them too.
    show_page(&f, cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::Tasks, cx));
    assert!(!f.ws.read_with(cx, |ws, _| ws.pr_list().is_shown()));

    // So does restoring a saved state, which never holds the page.
    use gpui::AppContext as _;
    show_page(&f, cx);
    let ws = f.ws.clone();
    cx.update_window(f.window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            let (state, projects) = ws.snapshot_for_disk(cx);
            ws.restore_from_disk(&state, &projects, window, cx);
        });
    })
    .unwrap();
    assert_eq!(f.ws.read_with(cx, |ws, _| ws.active_page()), None);
    assert!(!f.ws.read_with(cx, |ws, _| ws.pr_list().is_shown()));
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
    let vcx = &mut VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    let dock = f.ws.read_with(vcx, |ws, _| ws.docks.left.clone());
    let count = dock.read_with(vcx, |d, _| {
        let crate::workspace::layout::DockSnapshot::Left(snap) = &d.snap else {
            panic!("the left dock is staged");
        };
        Page::PullRequests.nav_count(snap)
    });
    assert_eq!(
        count,
        Some((1, crate::surface::strings::pull_requests::nav_count(1))),
        "the badge says what it counts, which is not a tab's count"
    );
}

#[gpui::test]
fn clearing_search_keeps_the_selected_pull_request_tab_and_its_counts(cx: &mut TestAppContext) {
    let (f, gh, _) = linked_window(cx);
    gh.set_pr_list(PrAudience::All, vec![item(7), item(8), item(9)]);
    show_page(&f, cx);
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    for (filter, total) in [
        (PrAudience::Mine, 2),
        (PrAudience::ReviewRequested, 1),
        (PrAudience::All, 3),
    ] {
        f.ws.update_in(&mut vcx, |ws, window, cx| {
            ws.set_pr_page_filter(filter, cx);
            let search = ws
                .pages
                .pull_requests
                .snapshot
                .as_ref()
                .unwrap()
                .search
                .clone();
            search.update(cx, |input, cx| {
                input.set_value("missing", window, cx);
            });
        });
        vcx.run_until_parked();
        f.ws.read_with(&vcx, |ws, _| {
            let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
            assert_eq!(snap.visible, 0, "{filter:?}: search narrows rows");
            assert_eq!(snap.counts, ["2", "1", "3"], "{filter:?}: tab counts");
        });
        let clear = vcx
            .debug_bounds("pr-clear-filters")
            .expect("filter reset action");
        vcx.simulate_click(clear.center(), Default::default());
        vcx.run_until_parked();
        f.ws.read_with(&vcx, |ws, _| {
            assert_eq!(ws.pr_page_filter(), filter, "{filter:?}: selected tab");
            let snap = ws.pages.pull_requests.snapshot.as_ref().unwrap();
            assert!(snap.query.is_empty(), "{filter:?}: search is cleared");
            assert_eq!(snap.visible, total, "{filter:?}: rows restored");
            assert_eq!(snap.counts, ["2", "1", "3"], "{filter:?}: tab counts");
        });
        assert!(
            vcx.debug_bounds("pr-clear-filters").is_none(),
            "{filter:?}: no search to reset"
        );
    }
    assert_eq!(search_totals(&gh), [1, 1, 1], "clearing is local");
}

#[gpui::test]
fn the_page_names_the_account_gh_searches_as(cx: &mut TestAppContext) {
    use daruda_github::availability::{CredentialSource, GhAccount};

    let (f, gh, _) = linked_window(cx);
    gh.set_account(
        "github.com",
        GhAccount {
            login: "octocat".into(),
            source: CredentialSource::Env("GH_TOKEN".into()),
        },
    );
    f.ws.update(cx, |ws, cx| ws.recheck_gh("github.com", cx));
    tick(cx);
    show_page(&f, cx);
    let accounts = f.ws.read_with(cx, |ws, cx| ws.pr_page_list(cx).accounts);
    assert_eq!(
        accounts,
        vec![(
            "github.com".to_owned(),
            GhAccount {
                login: "octocat".into(),
                source: CredentialSource::Env("GH_TOKEN".into()),
            }
        )]
    );
    assert_eq!(
        crate::surface::strings::pull_requests::accounts(&accounts),
        "github.com: signed in as octocat via the GH_TOKEN variable"
    );
}
