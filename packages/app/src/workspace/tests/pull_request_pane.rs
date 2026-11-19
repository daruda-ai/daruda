//! A pull request in a worktree's side tabs: one pane per worktree,
//! retargeted by the next pull request, on the same detail the page shows
//! for that pull request, and gone from the window's details once nothing
//! shows it.

use daruda_github::Gh as _;
use daruda_github::PrListItem;
use daruda_github::fake::{FakeGh, GhOp};
use daruda_store::project::LaneRef;
use gpui::{AppContext as _, TestAppContext, VisualTestContext};

use super::GitFixture;
use super::pull_request_detail::{detail, item, open, repo, window};
use crate::workspace::main_area::pane::PaneContent;
use crate::workspace::main_area::side_group::GroupsShown;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::detail::{PrDetailId, PrOpening};

/// The worktree with a shell tab, off the page, wide enough for both groups.
fn worktree(cx: &mut TestAppContext) -> (GitFixture, FakeGh, LaneRef) {
    let (f, gh, lane) = window(cx);
    gh.set_pr_detail(&repo(), 8, detail(8, "other"));
    act(&f, cx, |ws, window, cx| {
        ws.close_page(cx);
        if ws.active_runtime().tabs.is_empty() {
            ws.add_tab(window, cx);
        }
        ws.main_area.groups_width = 1400.0;
    });
    (f, gh, lane)
}

fn act(
    f: &GitFixture,
    cx: &mut TestAppContext,
    op: impl FnOnce(&mut super::Workspace, &mut gpui::Window, &mut gpui::Context<super::Workspace>),
) {
    let mut vcx = VisualTestContext::from_window(f.window.into(), cx);
    vcx.update(|window, cx| f.ws.update(cx, |ws, cx| op(ws, window, cx)));
    cx.run_until_parked();
}

fn open_pane(f: &GitFixture, lane: LaneRef, item: PrListItem, cx: &mut TestAppContext) {
    act(f, cx, move |ws, window, cx| {
        ws.open_pr_pane(lane, item, PrOpening::default(), window, cx);
        // Opening measures the test window; keep room for both groups.
        ws.main_area.groups_width = 1400.0;
    });
}

/// The detail each pull request pane of the worktree holds, by number.
fn panes(f: &GitFixture, cx: &mut TestAppContext) -> Vec<(PrDetailId, u64)> {
    f.ws.read_with(cx, |ws, _| {
        ws.active_runtime()
            .panes
            .iter()
            .filter_map(|pane| match &pane.content {
                PaneContent::PullRequest(pr) => Some(pr.detail),
                _ => None,
            })
            .map(|id| {
                let detail = ws.pages.pull_requests.details.get(id).expect("held detail");
                (id, detail.item().pr.number)
            })
            .collect()
    })
}

#[gpui::test]
fn a_pull_request_opens_beside_the_tabs_and_is_read_once(cx: &mut TestAppContext) {
    let (f, gh, lane) = worktree(cx);
    let reads = gh.count(GhOp::PrDetail);
    open_pane(&f, lane, item(7, "feat"), cx);

    let held = panes(&f, cx);
    assert_eq!(held.iter().map(|(_, n)| *n).collect::<Vec<_>>(), [7]);
    assert_eq!(gh.count(GhOp::PrDetail), reads + 1);
    f.ws.update(cx, |ws, cx| {
        assert!(ws.active_page().is_none());
        assert!(matches!(ws.groups_shown(), GroupsShown::Both { .. }));
        assert!(ws.keyboard_is_aside());
        assert_eq!(ws.shown_pr_pane_detail(), Some(held[0].0));
        // The pane draws the detail staged for it.
        ws.stage_pr_page(cx);
        let view = ws.pages.pull_requests.detail_snapshot.as_ref().unwrap();
        assert_eq!(view.id, held[0].0);
        // A pull request is read only beside the tabs.
        assert!(!ws.side_tab_can_go_back(0));
    });

    // Opening it again shows the same pane and reads nothing more.
    open_pane(&f, lane, item(7, "feat"), cx);
    assert_eq!(panes(&f, cx), held);
    assert_eq!(gh.count(GhOp::PrDetail), reads + 1);
}

#[gpui::test]
fn another_pull_request_retargets_the_worktrees_one_pane(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let first = panes(&f, cx)[0].0;
    open_pane(&f, lane, item(8, "other"), cx);

    let held = panes(&f, cx);
    assert_eq!(held.iter().map(|(_, n)| *n).collect::<Vec<_>>(), [8]);
    f.ws.read_with(cx, |ws, cx| {
        let runtime = ws.active_runtime();
        assert_eq!(runtime.side.tabs().len(), 1);
        let pane = runtime.side.active_pane().unwrap();
        let title = runtime
            .panes
            .iter()
            .find(|p| p.id == pane)
            .unwrap()
            .title(cx);
        assert_eq!(title.as_ref(), crate::surface::strings::git::pr_number(8));
        // The first detail ended with the pane that showed it.
        let details = &ws.pages.pull_requests.details;
        assert!(details.get(first).is_none());
        assert_eq!(details.len(), 1);
    });
}

#[gpui::test]
fn the_page_and_the_pane_show_one_detail_of_a_pull_request(cx: &mut TestAppContext) {
    let (f, gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let held = panes(&f, cx)[0].0;
    let reads = gh.count(GhOp::PrDetail);

    // The page's row for the same pull request shows the pane's detail.
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_page(), Some(Page::PullRequests));
        assert_eq!(ws.pr_detail_id(), Some(held));
        assert_eq!(ws.pages.pull_requests.details.len(), 1);
        assert_eq!(ws.shown_pr_pane_detail(), None, "the page covers the pane");
    });
    assert_eq!(gh.count(GhOp::PrDetail), reads, "nothing is read twice");

    // Back to the list: the pane still holds it.
    act(&f, cx, |ws, window, cx| ws.back_to_pr_list(window, cx));
    f.ws.read_with(cx, |ws, _| {
        assert!(ws.pr_detail_id().is_none());
        assert!(ws.pages.pull_requests.details.get(held).is_some());
    });

    // The pane closed: nothing shows it, so it ends.
    act(&f, cx, |ws, window, cx| {
        ws.close_page(cx);
        ws.request_close_side_tab(0, window, cx);
    });
    f.ws.read_with(cx, |ws, _| {
        assert!(ws.active_runtime().side.is_empty());
        assert_eq!(ws.pages.pull_requests.details.len(), 0);
    });
}

#[gpui::test]
fn a_pane_opens_its_pull_request_on_the_page(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let held = panes(&f, cx)[0].0;
    act(&f, cx, |ws, window, cx| {
        let pane = ws.active_runtime().side.active_pane().unwrap();
        ws.open_pr_pane_on_page(pane, window, cx);
    });
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_page(), Some(Page::PullRequests));
        assert_eq!(ws.pr_detail_id(), Some(held));
    });
}

/// The Git panel's entry reads the pull request, then shows it beside the
/// tabs where files open there, and on the page where they do not.
#[gpui::test]
fn the_git_panels_pull_request_opens_where_files_do(cx: &mut TestAppContext) {
    for (beside, on_page) in [(true, false), (false, true)] {
        let (f, gh, _lane) = worktree(cx);
        assert!(gh.pr_detail(&repo(), 7).is_ok());
        act(&f, cx, move |ws, window, cx| {
            ws.mirrors.file_viewer_open_beside = beside;
            ws.open_lane_pr(repo(), 7, window, cx);
        });
        cx.run_until_parked();
        f.ws.read_with(cx, |ws, _| {
            assert_eq!(ws.active_page().is_some(), on_page, "beside: {beside}");
            assert_eq!(ws.pr_detail_id().is_some(), on_page, "beside: {beside}");
        });
        assert_eq!(panes(&f, cx).len(), usize::from(beside), "beside: {beside}");
    }
}

#[gpui::test]
fn a_pull_request_pane_is_not_kept_across_a_restart(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let (_, projects) = f.ws.read_with(cx, |ws, cx| ws.snapshot_for_disk(cx));
    assert!(projects[0].lanes[0].side.is_none());
}

/// What the detail's buttons ask of GitHub is asked from a pane too: the
/// detail is found by its id, not as the one the page shows.
#[gpui::test]
fn a_panes_detail_acts_on_its_pull_request(cx: &mut TestAppContext) {
    let (f, gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let id = panes(&f, cx)[0].0;
    f.ws.read_with(cx, |ws, _| {
        assert!(ws.active_page().is_none());
        assert!(ws.pr_detail_id().is_none(), "the page shows none");
    });

    // Close asks first, as on the page: its dialog comes up.
    let asked = cx
        .update_window(f.window.into(), |_, window, cx| {
            use crate::ui::WindowExt as _;
            f.ws.update(cx, |ws, cx| ws.ask_pr_close(id, window, cx));
            let asked = window.has_active_dialog(cx);
            window.close_dialog(cx);
            asked
        })
        .unwrap();
    assert!(asked, "the pane's Close asks to confirm");
    cx.run_until_parked();

    let reruns = gh.count(GhOp::RerunFailed);
    f.ws.update(cx, |ws, cx| ws.rerun_pr_detail_checks(id, cx));
    cx.run_until_parked();
    assert_eq!(gh.count(GhOp::RerunFailed), reruns + 1);
}

/// Switching GitHub off closes every pull request pane with the page's
/// detail: none is left to run `gh`.
#[gpui::test]
fn switching_github_off_closes_the_pull_request_panes(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    act(&f, cx, |ws, window, cx| {
        ws.close_pr_detail_while_off(window, cx)
    });
    assert!(panes(&f, cx).is_empty());
    f.ws.read_with(cx, |ws, _| {
        assert!(ws.active_runtime().side.is_empty());
        assert_eq!(ws.pages.pull_requests.details.len(), 0);
        assert!(!ws.keyboard_is_aside());
    });
}

/// A worktree's runtime dropped whole (its project removed) lets go of the
/// detail its pane held.
#[gpui::test]
fn a_dropped_worktree_lets_go_of_its_pull_request(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    f.ws.update(cx, |ws, cx| {
        ws.release_pr_panes_of(|dropped| dropped.project == lane.project, cx);
        assert_eq!(ws.pages.pull_requests.details.len(), 0);
    });
}

/// The Git panel's pull request follows the rule a file does: with no tab
/// to be beside, it opens on the page.
#[gpui::test]
fn with_no_tab_to_be_beside_the_pull_request_opens_on_the_page(cx: &mut TestAppContext) {
    let (f, _gh, _lane) = worktree(cx);
    act(&f, cx, |ws, window, cx| {
        while !ws.active_runtime().tabs.is_empty() {
            ws.close_tab_at(0, window, cx);
        }
        ws.open_lane_pr(repo(), 7, window, cx);
    });
    cx.run_until_parked();
    assert!(panes(&f, cx).is_empty());
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_page(), Some(Page::PullRequests))
    });
}
