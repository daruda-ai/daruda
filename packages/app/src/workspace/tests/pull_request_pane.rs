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
use crate::workspace::main_area::pane::{PaneContent, PrPaneDetail, PrPaneRead};
use crate::workspace::main_area::side_group::GroupsShown;
use crate::workspace::pages::Page;
use crate::workspace::pages::pull_requests::detail::{PrDetailId, PrOpening, PrTab};

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
                PaneContent::PullRequest(pr) => Some(pr.detail.id().expect("an open detail")),
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

/// A pane that goes away takes its hold with it, whatever path removed
/// it: here its runtime is dropped whole, with no teardown called at all,
/// and the detail is over. What it leaves behind is settled afterwards.
#[gpui::test]
fn a_pane_dropped_without_teardown_still_ends_its_detail(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let id = panes(&f, cx)[0].0;
    f.ws.update(cx, |ws, cx| {
        *ws.active_runtime_mut() = crate::workspace::LaneRuntime::default();
        let details = &ws.pages.pull_requests.details;
        assert_eq!(details.len(), 0);
        assert!(details.get(id).is_none());
        assert!(ws.pr_detail(id).is_none(), "nothing acts on it any more");
        // Opening the pull request again makes a new detail, not the old.
        ws.settle_pr_details(cx);
    });
    open_pane(&f, lane, item(7, "feat"), cx);
    assert_ne!(panes(&f, cx)[0].0, id);
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

/// The worktree's pull request pane, closed the way its tab's button does.
fn close_pane(
    ws: &mut super::Workspace,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<super::Workspace>,
) {
    let pane = ws
        .active_runtime()
        .panes
        .iter()
        .find(|pane| pane.content.as_pull_request().is_some())
        .expect("a pull request pane")
        .id;
    let index = ws.active_runtime().side.position_of(pane).unwrap();
    ws.close_side_tab(index, window, cx);
}

/// What a closed pane's detail leaves is settled once the pane is gone:
/// the place to come back to is recorded, and what was typed is on disk.
#[gpui::test]
fn closing_the_pane_settles_what_its_detail_leaves(cx: &mut TestAppContext) {
    use crate::github::DraftStore;
    use daruda_store::review_drafts::load_review_drafts_in;
    let dir = tempfile::tempdir().unwrap();
    cx.update(|cx| DraftStore::install_at(dir.path().to_owned(), cx));
    let (f, gh, lane) = worktree(cx);
    gh.set_account(
        "github.com",
        daruda_github::availability::GhAccount {
            login: "me".into(),
            source: daruda_github::availability::CredentialSource::Env("GH_TOKEN".into()),
        },
    );
    f.ws.update(cx, |ws, cx| ws.recheck_gh("github.com", cx));
    cx.update(crate::github::PrPoller::tick);
    cx.run_until_parked();
    open_pane(&f, lane, item(7, "feat"), cx);
    let id = panes(&f, cx)[0].0;
    let key = f.ws.update(cx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx);
        ws.pages.pull_requests.details.get(id).unwrap().key()
    });
    let owner = cx.update(|cx| DraftStore::owner(&repo(), 7, cx)).unwrap();
    cx.update(|cx| DraftStore::set_reply(&owner, "thread", Some("Kept".into()), cx));
    assert!(
        load_review_drafts_in(dir.path()).unwrap().prs.is_empty(),
        "not written while it is being typed"
    );

    act(&f, cx, close_pane);
    f.ws.read_with(cx, |ws, _| {
        let page = &ws.pages.pull_requests;
        assert_eq!(page.details.len(), 0);
        assert_eq!(
            page.progress.place(&key).map(|place| place.tab),
            Some(PrTab::Description)
        );
    });
    assert_eq!(load_review_drafts_in(dir.path()).unwrap().prs.len(), 1);
}

/// Closed and opened again within one update, the pull request comes back
/// to the tab it was left at: what the closed detail leaves is settled
/// before the next one is made, not only once the update is over.
#[gpui::test]
fn a_pull_request_opened_again_at_once_comes_back_to_its_place(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let id = panes(&f, cx)[0].0;
    f.ws.update(cx, |ws, cx| {
        ws.select_pr_detail_tab(id, PrTab::Description, cx)
    });
    cx.run_until_parked();

    act(&f, cx, move |ws, window, cx| {
        close_pane(ws, window, cx);
        ws.open_pr_pane(lane, item(7, "feat"), PrOpening::default(), window, cx);
    });
    let again = panes(&f, cx)[0].0;
    assert_ne!(again, id, "a new detail");
    f.ws.read_with(cx, |ws, _| {
        let detail = ws.pages.pull_requests.details.get(again).unwrap();
        assert_eq!(detail.tab(), PrTab::Description);
    });
}

/// A runtime dropped whole takes its pull request pane with it: the
/// teardown every such path shares settles what the pane's detail leaves.
#[gpui::test]
fn tearing_a_runtime_down_whole_settles_its_pull_request(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let id = panes(&f, cx)[0].0;
    let key = f.ws.read_with(cx, |ws, _| {
        ws.pages.pull_requests.details.get(id).unwrap().key()
    });
    act(&f, cx, move |ws, window, cx| {
        ws.forget_runtime_panes(|dropped| *dropped == lane, window, cx);
        *ws.active_runtime_mut() = crate::workspace::LaneRuntime::default();
    });
    f.ws.read_with(cx, |ws, _| {
        let page = &ws.pages.pull_requests;
        assert_eq!(page.details.len(), 0);
        assert!(page.progress.place(&key).is_some(), "its place is kept");
    });
}

/// What the workspace asks of every pane, answered by a shell and by a
/// pull request pane: where each may sit, and what draws its focus.
#[gpui::test]
fn each_pane_kind_answers_for_itself(cx: &mut TestAppContext) {
    use crate::workspace::main_area::pane::behavior::PaneHosting;
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    f.ws.read_with(cx, |ws, cx| {
        // (kind, hosting, the wrapper tracks its focus, an account, saves)
        let cases = [
            ("a shell", PaneHosting::Tabs, false, true, false),
            ("a pull request", PaneHosting::Side, true, false, false),
        ];
        let panes = &ws.active_runtime().panes;
        assert_eq!(panes.len(), cases.len());
        for (pane, (label, hosting, wrapped, account, saves)) in panes.iter().zip(cases) {
            let kind = pane.content.behavior();
            assert_eq!(kind.hosting(), hosting, "{label}");
            assert_eq!(kind.wrapper_focus_handle().is_some(), wrapped, "{label}");
            assert_eq!(kind.account_selection().is_some(), account, "{label}");
            assert_eq!(kind.can_save(), saves, "{label}");
            assert!(!kind.is_dirty(cx), "{label}");
            assert!(!kind.title(cx).is_empty(), "{label}");
        }
    });
}

/// A pull request of the stack is opened where the stack is being read:
/// from the worktree's pane it takes the pane, and the page stays closed.
#[gpui::test]
fn a_stack_entry_pressed_in_the_pane_opens_in_the_pane(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let id = panes(&f, cx)[0].0;
    act(&f, cx, move |ws, window, cx| {
        ws.open_stack_entry(id, repo(), 8, window, cx);
        ws.main_area.groups_width = 1400.0;
    });
    let held = panes(&f, cx);
    assert_eq!(held.iter().map(|(_, n)| *n).collect::<Vec<_>>(), [8]);
    f.ws.read_with(cx, |ws, _| assert!(ws.active_page().is_none()));

    // Pressed on the page, it is the page's detail that changes.
    let on_page = held[0].0;
    act(&f, cx, move |ws, window, cx| {
        let pane = ws
            .active_runtime()
            .panes
            .iter()
            .find(|pane| pane.content.as_pull_request().is_some())
            .unwrap()
            .id;
        ws.open_pr_pane_on_page(pane, window, cx);
        ws.open_stack_entry(on_page, repo(), 7, window, cx);
    });
    f.ws.read_with(cx, |ws, _| {
        assert_eq!(ws.active_page(), Some(Page::PullRequests));
        let shown = ws
            .pages
            .pull_requests
            .detail()
            .expect("a detail on the page");
        assert_eq!(shown.item().pr.number, 7);
    });
    assert_eq!(panes(&f, cx)[0].1, 8, "the pane keeps its own");
}

/// The page's button to the pull request's worktree takes the pull request
/// along: it is read there beside the tabs, on the detail the page showed.
#[gpui::test]
fn going_to_the_worktree_from_the_page_takes_the_pull_request_along(cx: &mut TestAppContext) {
    let (f, _gh, _lane) = worktree(cx);
    f.ws.update(cx, |ws, cx| ws.show_page(Page::PullRequests, cx));
    open(&f, item(7, "feat"), cx);
    cx.run_until_parked();
    let id =
        f.ws.read_with(cx, |ws, _| ws.pages.pull_requests.detail().unwrap().id());
    act(&f, cx, move |ws, window, cx| {
        ws.open_pr_detail_lane(id, window, cx);
        ws.main_area.groups_width = 1400.0;
    });
    assert_eq!(panes(&f, cx), [(id, 7)]);
    f.ws.read_with(cx, |ws, _| {
        assert!(ws.active_page().is_none());
        assert!(ws.keyboard_is_aside());
    });
}

/// The saved layout and a new window restored from it, its reads not
/// landed yet.
fn restart_unparked(
    f: &GitFixture,
    cx: &mut TestAppContext,
) -> gpui::WindowHandle<super::Workspace> {
    let (workspace_state, project_states) = f.ws.read_with(cx, |ws, cx| ws.snapshot_for_disk(cx));
    let config = daruda_config::Config::default();
    cx.add_window(|window, cx| {
        let mut ws = super::Workspace::new_with_project_for_test(
            &config,
            None,
            super::fresh_test_data_dir(),
            window,
            cx,
        );
        ws.restore_from_disk(&workspace_state, &project_states, window, cx);
        ws
    })
}

/// The saved layout and a new window restored from it, its reads landed.
fn restart(f: &GitFixture, cx: &mut TestAppContext) -> gpui::Entity<super::Workspace> {
    let window = restart_unparked(f, cx);
    cx.run_until_parked();
    window.root(cx).unwrap()
}

/// The restored worktree's pull request pane: its number and how far its
/// read got.
fn restored_pane(
    ws: &gpui::Entity<super::Workspace>,
    cx: &mut TestAppContext,
) -> Option<(u64, Option<PrPaneRead>)> {
    ws.read_with(cx, |ws, _| {
        ws.active_runtime().panes.iter().find_map(|pane| {
            let pr = pane.content.as_pull_request()?;
            let read = match &pr.detail {
                PrPaneDetail::Open(_) => None,
                PrPaneDetail::Unread(read) => Some(read.clone()),
            };
            Some((pr.number, read))
        })
    })
}

/// The layout keeps which pull request the pane showed, not what it
/// showed: after a restart the pane is where it was and reads it again,
/// without taking the keyboard.
#[gpui::test]
fn a_restart_reads_the_pull_request_pane_again(cx: &mut TestAppContext) {
    let (f, gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let (_, project_states) = f.ws.read_with(cx, |ws, cx| ws.snapshot_for_disk(cx));
    let side = project_states[0].lanes[0]
        .side
        .as_ref()
        .expect("side tabs saved");
    assert!(matches!(
        &side.tabs[..],
        [daruda_store::project::SerializedTab {
            layout: daruda_store::project::SerializedLayout::Leaf {
                content: daruda_store::project::SerializedPaneContent::PullRequest(pr),
                ..
            },
            ..
        }] if pr.number == 7 && (pr.owner.as_str(), pr.name.as_str()) == (repo().owner(), repo().name())
    ));
    let reads = gh.count(GhOp::PrDetail);

    let restored = restart(&f, cx);
    assert_eq!(restored_pane(&restored, cx), Some((7, None)));
    assert_eq!(gh.count(GhOp::PrDetail), reads + 1);
    restored.read_with(cx, |ws, _| {
        let id = ws
            .active_runtime()
            .panes
            .iter()
            .find_map(|p| p.content.as_pull_request()?.detail.id());
        let detail = ws.pages.pull_requests.details.get(id.unwrap()).unwrap();
        assert_eq!(detail.item().pr.number, 7);
        assert!(!ws.keyboard_is_aside());
    });
}

/// A read that fails stays in the pane, which offers it again.
#[gpui::test]
fn a_restored_pane_that_cannot_read_says_so_and_tries_again(cx: &mut TestAppContext) {
    let (f, gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    gh.fail(GhOp::PrDetail, daruda_github::fake::GhFailure::Timeout);

    let restored = restart(&f, cx);
    assert!(matches!(
        restored_pane(&restored, cx),
        Some((7, Some(PrPaneRead::Failed(_))))
    ));

    gh.clear_failure(GhOp::PrDetail);
    restored.update(cx, |ws, cx| {
        let pane = ws
            .active_runtime()
            .panes
            .iter()
            .find(|pane| pane.content.as_pull_request().is_some())
            .unwrap()
            .id;
        ws.retry_pr_pane_read(pane, cx);
    });
    cx.run_until_parked();
    assert_eq!(restored_pane(&restored, cx), Some((7, None)));
}

/// With GitHub off, a saved pull request pane does not come back.
#[gpui::test]
fn with_github_off_the_pane_is_not_restored(cx: &mut TestAppContext) {
    let (f, gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);
    let reads = gh.count(GhOp::PrDetail);
    super::pull_request_detail::set_github_enabled(cx, false);

    let restored = restart(&f, cx);
    assert_eq!(restored_pane(&restored, cx), None);
    assert_eq!(gh.count(GhOp::PrDetail), reads);
}

/// Another pull request opened while the restored one is still being read
/// takes the pane; the read lands on nothing.
#[gpui::test]
fn a_read_landing_after_the_pane_moved_on_is_dropped(cx: &mut TestAppContext) {
    let (f, _gh, lane) = worktree(cx);
    open_pane(&f, lane, item(7, "feat"), cx);

    let window = restart_unparked(&f, cx);
    let restored = window.root(cx).unwrap();
    assert!(matches!(
        restored_pane(&restored, cx),
        Some((7, Some(PrPaneRead::Reading)))
    ));
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.update(|window, cx| {
        restored.update(cx, |ws, cx| {
            let lane = ws.active_ref();
            ws.main_area.groups_width = 1400.0;
            ws.open_pr_pane(lane, item(8, "other"), PrOpening::default(), window, cx);
        })
    });
    cx.run_until_parked();
    assert_eq!(restored_pane(&restored, cx), Some((8, None)));
    restored.read_with(cx, |ws, _| {
        let held = ws
            .active_runtime()
            .panes
            .iter()
            .find_map(|pane| pane.content.as_pull_request()?.detail.id())
            .and_then(|id| ws.pages.pull_requests.details.get(id))
            .map(|detail| detail.item().pr.number);
        assert_eq!(held, Some(8), "the pane holds the detail it moved on to");
        assert_eq!(ws.pages.pull_requests.details.len(), 1, "no detail for 7");
    });
}
