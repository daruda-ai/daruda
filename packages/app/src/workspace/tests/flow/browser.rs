//! Flow browsing never borrows the active worktree as an action target.

use super::*;
use crate::workspace::{
    flow_browser::{FlowGrouping, FlowScope, FlowTab, RunFilter},
    flow_paths,
    pages::Page,
};
use daruda_store::project::LaneRef;
use gpui::{Modifiers, VisualTestContext};

// GPUI's test selector API requires static strings, including path-based ids.
fn selector(prefix: &str, path: &std::path::Path) -> &'static str {
    Box::leak(format!("{prefix}{}", path.display()).into_boxed_str())
}

fn other_project(
    ws: &gpui::Entity<Workspace>,
    cx: &mut TestAppContext,
    flow: &str,
) -> (tempfile::TempDir, LaneRef, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let flows = flow_paths::flows_dir(dir.path());
    std::fs::create_dir_all(&flows).unwrap();
    let path = flows.join("other.yaml");
    std::fs::write(&path, flow).unwrap();
    let target = ws.update(cx, |ws, _| {
        let id = ws.next_project_id;
        ws.next_project_id += 1;
        let project = crate::project::Project::bootstrap_placeholder(id, dir.path().to_path_buf());
        let target = LaneRef {
            project: id,
            lane: project.lanes[0].id,
        };
        ws.projects.push(project);
        ws.main_area.runtimes.entry(target).or_default();
        target
    });
    (dir, target, path)
}

/// An event names its directory, so it leaves other worktrees' listings
/// alone. A worktree the page stops browsing is no longer watched, so when
/// the page browses it again its listing is read again rather than trusted.
#[gpui::test]
async fn flow_listings_follow_their_own_directories_and_the_watch(cx: &mut TestAppContext) {
    let (_dir, ws, original_file, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let (_other, target, other_file) = other_project(&ws, cx, ONE_AGENT);
    let active = ws.read_with(cx, |ws, _| ws.active);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    let fresh = |ws: &Workspace, lane: LaneRef| {
        ws.lane_scoped
            .get(&lane)
            .is_some_and(|state| state.flows.list.get(lane).is_some())
    };
    for scope in [target, active] {
        ws.update_in(&mut vcx, |ws, window, cx| {
            ws.open_page(Page::Flows, window, cx);
            ws.set_flow_scope(FlowScope::Worktree(scope), cx);
            ws.flow_list_for_panel(cx);
        });
        vcx.run_until_parked();
    }

    let event = crate::hooks::flow_watcher::FlowsEvent::In(vec![
        original_file.parent().unwrap().to_path_buf(),
    ]);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.apply_flows_event(&event, window, cx);
        assert!(!fresh(ws, active), "the changed directory is read again");
        assert!(fresh(ws, target), "another worktree's listing stays");
    });

    // Written while `target` is not browsed, so no event reaches it.
    let added = other_file.with_file_name("added.yaml");
    std::fs::write(&added, ONE_AGENT).unwrap();
    ws.update(&mut vcx, |ws, cx| {
        ws.set_flow_scope(FlowScope::Worktree(target), cx);
        ws.flow_list_for_panel(cx);
    });
    vcx.run_until_parked();
    let listed = ws.update(&mut vcx, |ws, cx| ws.flow_list_for_panel(cx));
    assert!(
        listed.iter().any(|file| file.path == added),
        "browsed again, read again: {listed:?}"
    );
}

#[gpui::test]
async fn flow_browser_scope_filters_without_switching_the_active_worktree(cx: &mut TestAppContext) {
    let (_dir, ws, original_file, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let (_other, target, other_file) = other_project(&ws, cx, ONE_AGENT);
    let active = ws.read_with(cx, |ws, _| ws.active);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        ws.set_flow_scope(FlowScope::Worktree(target), cx);
        ws.flow_list_for_panel(cx);
        assert_eq!(ws.active, active);
    });
    vcx.run_until_parked();
    let listed = ws.update(&mut vcx, |ws, cx| ws.flow_list_for_panel(cx));
    assert!(listed.iter().any(|file| file.path == other_file));
    assert!(!listed.iter().any(|file| file.path == original_file));
    let source = vcx
        .debug_bounds("flow-origin-1")
        .expect("repository filter");
    vcx.simulate_click(source.center(), Modifiers::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(
            ws.flows.browser.state.origin,
            Some(flow_paths::FlowOrigin::Repo)
        );
        assert_eq!(ws.active, active);
        assert_eq!(ws.flow_browser_lane(), target);
    });
    let clear = vcx.debug_bounds("flow-clear-filters").unwrap();
    vcx.simulate_click(clear.center(), Modifiers::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.flows.browser.state.origin, None);
        assert_eq!(ws.flow_browser_lane(), target);
        assert_eq!(ws.active, active);
    });
}

#[gpui::test]
async fn flow_browser_run_button_keeps_its_target_through_profile_selection(
    cx: &mut TestAppContext,
) {
    let (_dir, ws, _path, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let (_other, target, path) = other_project(&ws, cx, WITH_PROFILES);
    let active = ws.read_with(cx, |ws, _| ws.active);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    vcx.simulate_resize(gpui::size(gpui::px(1280.0), gpui::px(800.0)));
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        ws.set_task_scope(daruda_store::tasks::TaskScope::AllProjects, cx);
    });
    vcx.run_until_parked();
    let button = vcx
        .debug_bounds(selector("flow-run-", &path))
        .expect("run button");
    vcx.simulate_click(button.center(), Modifiers::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.active, active);
        assert_eq!(ws.active_page(), Some(Page::Flows), "Run did not open the graph");
        assert!(matches!(ws.overlays.flow_picker.focused_pick(),
            Some(crate::workspace::command::flow_picker::FlowPick::Profile { lane, .. }) if lane == target));
    });
}

#[gpui::test]
async fn flow_browser_creation_writes_only_to_the_selected_project(cx: &mut TestAppContext) {
    let (_dir, ws, _path, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let (_other, target, _path) = other_project(&ws, cx, ONE_AGENT);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        let original_root = ws.active_project().unwrap().root.clone();
        let target_root = ws.project_for(target.project).unwrap().root.clone();
        let wrong = flow_paths::project_flows_dir(&ws.data_dir, &original_root);
        let right = flow_paths::project_flows_dir(&ws.data_dir, &target_root);
        ws.set_flow_scope(FlowScope::Worktree(target), cx);
        ws.create_flow_in(target, "scoped", window, cx);
        assert!(right.is_dir());
        assert!(!wrong.exists());
        let active = ws.active;
        assert_ne!(
            active, target,
            "the page opens the graph; the worktree stays"
        );
        let (_, lane, view) = ws.open_graph().expect("the new flow is the page's graph");
        assert_eq!(
            lane, target,
            "and the graph runs in the worktree it was made for"
        );
        assert_eq!(view.read(cx).path().parent(), Some(right.as_path()));
    });
}

#[gpui::test]
async fn flow_browser_tabs_retain_separate_queries_and_scroll_offsets(cx: &mut TestAppContext) {
    let (_dir, ws, _path, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        ws.flows.browser.searches[0].update(cx, |input, cx| input.set_value("ship", window, cx));
        ws.flows.browser.scrolls[0].set_offset(gpui::point(gpui::px(0.0), gpui::px(-80.0)));
        ws.set_flow_tab(FlowTab::Runs, cx);
        ws.flows.browser.searches[1].update(cx, |input, cx| input.set_value("other", window, cx));
        ws.flows.browser.scrolls[1].set_offset(gpui::point(gpui::px(0.0), gpui::px(-20.0)));
        ws.set_flow_tab(FlowTab::Definitions, cx);
        assert_eq!(ws.flow_browser_snapshot(cx).query, "ship");
        assert_eq!(
            ws.workspace_page.state().unwrap().scroll.offset().y,
            gpui::px(-80.0)
        );
        ws.close_page(cx);
        ws.open_page(Page::Flows, window, cx);
        assert_eq!(
            ws.workspace_page.state().unwrap().scroll.offset().y,
            gpui::px(-80.0)
        );
        ws.set_flow_tab(FlowTab::Runs, cx);
        assert_eq!(ws.flow_browser_snapshot(cx).query, "other");
        assert_eq!(
            ws.workspace_page.state().unwrap().scroll.offset().y,
            gpui::px(-20.0)
        );
    });
}

#[gpui::test]
async fn flow_browser_table_alignment_and_grouping_survive_narrow_windows(cx: &mut TestAppContext) {
    let (_dir, ws, path, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        ws.set_flow_grouping(FlowGrouping::Origin, cx);
    });
    vcx.run_until_parked();
    for width in [800.0, 1280.0] {
        vcx.simulate_resize(gpui::size(gpui::px(width), gpui::px(800.0)));
        vcx.run_until_parked();
        let heading = vcx.debug_bounds("flow-source-column").unwrap();
        let source = vcx.debug_bounds(selector("flow-source-", &path)).unwrap();
        let title = vcx.debug_bounds(selector("flow-title-", &path)).unwrap();
        assert_eq!(heading.left(), source.left());
        assert!(title.right() <= source.left());
        assert!(title.size.width >= gpui::px(crate::ui::theme::list_metrics::TITLE_MIN_W));
    }
    for open in [false, true] {
        let group = vcx.debug_bounds("flow-group-Repo").unwrap();
        vcx.simulate_click(group.center(), Modifiers::default());
        vcx.run_until_parked();
        assert_eq!(
            vcx.debug_bounds(selector("flow-file-", &path)).is_some(),
            open
        );
    }
    let file = vcx.debug_bounds(selector("flow-title-", &path)).unwrap();
    vcx.simulate_click(file.center(), Modifiers::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(
            ws.active_page(),
            Some(Page::Flows),
            "the graph opens in the page"
        );
        assert!(ws.open_graph().is_some());
    });
    let back = vcx
        .debug_bounds("flow-back-to-list")
        .expect("editor return action");
    vcx.simulate_click(back.center(), Modifiers::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.active_page(), Some(Page::Flows));
        assert!(ws.open_graph().is_none(), "back is the list");
        assert_eq!(ws.flows.browser.state.grouping, FlowGrouping::Origin);
    });
}

#[gpui::test]
async fn flow_browser_request_recovery_does_not_clear_definition_filters(cx: &mut TestAppContext) {
    let (_dir, ws, _path, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        ws.set_flow_origin(Some(flow_paths::FlowOrigin::Repo), cx);
        ws.flows.browser.searches[0].update(cx, |input, cx| input.set_value("ship", window, cx));
        ws.flows.browser.searches[1].update(cx, |input, cx| input.set_value("hidden", window, cx));
        ws.show_flow_questions(window, cx);
        assert_eq!(ws.flows.browser.state.tab, FlowTab::Runs);
        assert_eq!(ws.flows.browser.state.run_filter, RunFilter::Asking);
        assert!(ws.flow_browser_snapshot(cx).query.is_empty());
        assert_eq!(
            ws.flows.browser.state.origin,
            Some(flow_paths::FlowOrigin::Repo)
        );
        assert_eq!(
            ws.flows.browser.searches[0].read(cx).value().as_ref(),
            "ship"
        );
    });
}

#[gpui::test]
async fn flow_browser_narrow_tables_scroll_to_the_action_column(cx: &mut TestAppContext) {
    use crate::workspace::flow_history::{FlowHistory, FlowRunEntry};
    use gpui::{ScrollDelta, ScrollEvent, TouchPhase, point, px};

    let (_dir, ws, path, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    vcx.simulate_resize(gpui::size(px(800.0), px(600.0)));
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        let lane = ws.active;
        ws.lane_scoped_mut(lane).flows.history.put(
            lane,
            FlowHistory::seeded(vec![FlowRunEntry {
                dir: path.with_file_name("run-01"),
                started: "now".into(),
                report: None,
                status: daruda_flow::marker::RunStatus::Crashed,
            }]),
        );
    });
    for tab in [FlowTab::Definitions, FlowTab::Runs] {
        ws.update(&mut vcx, |ws, cx| ws.set_flow_tab(tab, cx));
        vcx.run_until_parked();
        let viewport = vcx.debug_bounds("flow-table-scroll").unwrap();
        let before = vcx.debug_bounds("flow-actions-column").unwrap();
        assert!(
            before.right() > viewport.right(),
            "{tab:?}: action={before:?}, viewport={viewport:?}"
        );
        vcx.simulate_event(ScrollEvent {
            source: gpui::ScrollSource::Pointer,
            position: viewport.center(),
            delta: ScrollDelta::Pixels(point(px(-1000.0), px(0.0))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Started,
        });
        vcx.run_until_parked();
        let after = vcx.debug_bounds("flow-actions-column").unwrap();
        assert!(
            after.right() <= viewport.right(),
            "{tab:?} actions must be reachable"
        );
        assert!(after.left() >= viewport.left());
    }
}

#[gpui::test]
async fn aggregate_flow_lists_retain_both_projects_and_independent_read_claims(
    cx: &mut TestAppContext,
) {
    let (_dir, ws, original_file, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let (_other, target, other_file) = other_project(&ws, cx, ONE_AGENT);
    let active = ws.read_with(cx, |ws, _| ws.active);
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.set_task_scope(daruda_store::tasks::TaskScope::AllProjects, cx);
        ws.open_page(Page::Flows, window, cx);
    });
    vcx.run_until_parked();
    ws.update(&mut vcx, |ws, cx| {
        ws.stage_flow_page(cx);
        let groups = &ws.flows.browser.page_snapshot.as_ref().unwrap().groups;
        assert!(groups.iter().any(|group| group.lane == active
            && group.files.iter().any(|file| file.path == original_file)));
        assert!(
            groups.iter().any(|group| group.lane == target
                && group.files.iter().any(|file| file.path == other_file))
        );
        for lane in [active, target] {
            let cache = &mut ws.lane_scoped_mut(lane).flows.list;
            assert_eq!(
                cache.begin_read(lane),
                None,
                "an unchanged aggregate page must not reread {lane:?}"
            );
        }
        ws.invalidate_flow_history(target);
        assert!(
            ws.lane_scoped
                .get(&active)
                .unwrap()
                .flows
                .history
                .get(active)
                .is_some()
        );
        assert_eq!(ws.active, active);
    });
}

#[gpui::test]
async fn execution_history_back_and_escape_restore_the_definition_and_browser(
    cx: &mut TestAppContext,
) {
    use crate::workspace::pages::flows::{
        detail::FlowDetailBody,
        execution_detail::ExecutionContent,
        execution_fixture::{self, ExecutionShot},
    };
    let (root, ws, original, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let path = flow_paths::flows_dir(root.path()).join("release.yaml");
    std::fs::copy(original, &path).unwrap();
    let recorded = execution_fixture::seed(root.path(), ExecutionShot::Completed);
    let runs = flow_paths::runs_dir(root.path());
    std::fs::create_dir_all(&runs).unwrap();
    let dir = runs.join(recorded.file_name().unwrap());
    std::fs::rename(recorded, &dir).unwrap();
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    let active = ws.read_with(&vcx, |ws, _| ws.active);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.set_flow_origin(Some(flow_paths::FlowOrigin::Repo), cx);
        ws.flows.browser.searches[FlowTab::Definitions.index()]
            .update(cx, |input, cx| input.set_value("release", window, cx));
        ws.open_flow_graph(active, &path, window, cx);
        ws.open_graph_execution_history(active, &path, window, cx);
    });
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, cx| {
        let detail = ws
            .pages
            .flows
            .detail
            .as_ref()
            .expect("the selected flow's execution");
        let FlowDetailBody::Run(run) = &detail.body else {
            panic!("execution");
        };
        assert_eq!(run.dir, dir);
        assert!(
            run.execution.source_only,
            "definition history keeps its source filter"
        );
        assert_eq!(run.return_to.as_ref().unwrap().view.read(cx).path(), path);
    });
    let key = gpui::KeyDownEvent {
        keystroke: gpui::Keystroke {
            modifiers: Default::default(),
            key: "escape".into(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    };
    for escape in [false, true] {
        ws.update_in(&mut vcx, |ws, window, cx| {
            let id = ws.flow_detail_id().unwrap();
            ws.show_execution_report(id, cx);
            if escape {
                ws.on_page_key_down(&key, window, cx);
            } else {
                ws.back_from_execution(window, cx);
            }
            let FlowDetailBody::Run(run) = &ws.pages.flows.detail.as_ref().unwrap().body else {
                panic!("execution");
            };
            assert!(run.execution.content == ExecutionContent::Attempts);
            if escape {
                ws.on_page_key_down(&key, window, cx);
            } else {
                ws.back_from_execution(window, cx);
            }
            assert!(ws.open_graph_of(&path, cx).is_some());
            assert_eq!(ws.flows.browser.state.tab, FlowTab::Definitions);
            assert_eq!(
                ws.flows.browser.state.origin,
                Some(flow_paths::FlowOrigin::Repo)
            );
            assert_eq!(
                ws.flows.browser.searches[FlowTab::Definitions.index()]
                    .read(cx)
                    .value(),
                "release"
            );
            assert_eq!(ws.active, active);
            if !escape {
                ws.open_graph_execution_history(active, &path, window, cx);
            }
        });
        vcx.run_until_parked();
    }
}

#[gpui::test]
async fn a_past_run_without_a_report_still_opens_execution_evidence(cx: &mut TestAppContext) {
    use crate::workspace::pages::flows::{
        detail::FlowDetailBody,
        execution_detail::Inspection,
        execution_fixture::{self, ExecutionShot},
    };
    let (root, ws, _, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let recorded = execution_fixture::seed(root.path(), ExecutionShot::Completed);
    let runs = flow_paths::runs_dir(root.path());
    std::fs::create_dir_all(&runs).unwrap();
    let dir = runs.join(recorded.file_name().unwrap());
    std::fs::rename(recorded, &dir).unwrap();
    let report = dir.join(daruda_flow::record::RUN_REPORT_FILE);
    if report.exists() {
        std::fs::remove_file(report).unwrap();
    }
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Flows, window, cx);
        ws.set_flow_tab(FlowTab::Runs, cx);
        ws.flow_history_for_panel(cx);
    });
    vcx.run_until_parked();
    let bounds = vcx
        .debug_bounds(selector("flow-run-row-", &dir))
        .expect("recorded row");
    vcx.simulate_click(bounds.center(), Default::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        let FlowDetailBody::Run(run) = &ws.pages.flows.detail.as_ref().expect("run detail").body
        else {
            panic!("run detail")
        };
        assert_eq!(run.dir, dir);
        assert!(matches!(run.execution.inspection, Inspection::Ready(_)));
    });
}

#[gpui::test]
async fn starting_from_the_graph_opens_execution_and_back_keeps_the_exact_editor(
    cx: &mut TestAppContext,
) {
    use crate::workspace::pages::flows::detail::FlowDetailBody;
    // The real run log wakes the app from its own thread.
    cx.executor().allow_parking();
    let (root, ws, path, wh) = workspace_with_a_flow(
        cx,
        "version: 1\nnodes:\n  - id: done\n    kind: command\n    run: \"true\"\n",
    );
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    let (id, view) = ws.update_in(&mut vcx, |ws, window, cx| {
        let lane = ws.active;
        ws.open_flow_graph(lane, &path, window, cx);
        let (id, _, view) = ws.open_graph().unwrap();
        let name = view.read(cx).name_input_for_test();
        name.update(cx, |input, cx| {
            input.set_value("Retained draft name", window, cx)
        });
        ws.run_flow_from_graph(id, None, window, cx);
        assert!(matches!(
            ws.pages.flows.detail.as_ref().unwrap().body,
            FlowDetailBody::Run(_)
        ));
        (id, view)
    });
    assert!(
        wait_for_run(&mut vcx, |cx| !run_is_held(cx, root.path())).await,
        "the run settles before its fixture is removed"
    );
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.close_page_detail(window, cx);
    });
    assert!(
        vcx.has_pending_prompt(),
        "execution still protects its retained graph draft"
    );
    vcx.simulate_prompt_answer(&crate::surface::strings::common::btn_cancel());
    vcx.run_until_parked();
    ws.update_in(&mut vcx, |ws, window, cx| {
        assert!(matches!(
            ws.pages.flows.detail.as_ref().unwrap().body,
            FlowDetailBody::Run(_)
        ));
        ws.back_from_execution(window, cx);
        let (restored, _, graph) = ws.open_graph().unwrap();
        assert_eq!(restored, id);
        assert_eq!(graph, view);
        assert_eq!(graph.read(cx).edited_name(cx), "Retained draft name");
        ws.close_page_detail(window, cx);
    });
    assert!(
        vcx.has_pending_prompt(),
        "retained draft still participates in the leave guard"
    );
}

#[gpui::test]
async fn switching_execution_history_keeps_the_definition_draft_and_return_path(
    cx: &mut TestAppContext,
) {
    use crate::workspace::pages::flows::{
        detail::FlowDetailBody,
        execution_fixture::{self, ExecutionShot},
    };
    let (root, ws, original, wh) = workspace_with_a_flow(cx, ONE_AGENT);
    let path = flow_paths::flows_dir(root.path()).join("release.yaml");
    std::fs::copy(original, &path).unwrap();
    let runs = flow_paths::runs_dir(root.path());
    std::fs::create_dir_all(&runs).unwrap();
    let dirs = [
        "000001a1817fab35-0000e459-0001",
        "000001a1817fab36-0000e459-0002",
    ]
    .map(|name| {
        let seeded = execution_fixture::seed(root.path(), ExecutionShot::Completed);
        let dir = runs.join(name);
        std::fs::rename(seeded, &dir).unwrap();
        dir
    });
    let mut vcx = VisualTestContext::from_window(wh.into(), cx);
    let (lane, graph_id, graph) = ws.update_in(&mut vcx, |ws, window, cx| {
        let lane = ws.active;
        ws.open_flow_graph(lane, &path, window, cx);
        let (id, _, view) = ws.open_graph().unwrap();
        view.read(cx).name_input_for_test().update(cx, |input, cx| {
            input.set_value("Unsaved definition", window, cx)
        });
        ws.open_graph_execution_history(lane, &path, window, cx);
        (lane, id, view)
    });
    vcx.run_until_parked();
    for dir in [&dirs[0], &dirs[1]] {
        ws.update_in(&mut vcx, |ws, window, cx| {
            let id = ws.flow_detail_id().unwrap();
            ws.set_execution_filter(id, RunFilter::Done, cx);
            let FlowDetailBody::Run(run) = &ws.pages.flows.detail.as_ref().unwrap().body else {
                panic!("execution")
            };
            if run.execution.source_only {
                ws.toggle_execution_source(id, cx);
            }
            ws.open_run_detail(lane, dir, window, cx);
        });
        vcx.run_until_parked();
        assert!(
            !vcx.has_pending_prompt(),
            "switching records does not leave the definition"
        );
        ws.read_with(&vcx, |ws, cx| {
            let FlowDetailBody::Run(run) = &ws.pages.flows.detail.as_ref().unwrap().body else {
                panic!("execution")
            };
            assert_eq!(&run.dir, dir);
            assert!(!run.execution.source_only);
            assert_eq!(run.execution.filter, RunFilter::Done);
            let parent = run.return_to.as_ref().expect("the retained definition");
            assert_eq!(parent.id, graph_id);
            assert_eq!(parent.view, graph);
            assert_eq!(parent.view.read(cx).edited_name(cx), "Unsaved definition");
        });
    }
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.close_page_detail(window, cx);
    });
    assert!(
        vcx.has_pending_prompt(),
        "leaving the whole definition still guards its draft"
    );
    vcx.simulate_prompt_answer(&crate::surface::strings::common::btn_cancel());
    vcx.run_until_parked();
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.back_from_execution(window, cx);
        assert_eq!(ws.flow_detail_id(), Some(graph_id));
        assert_eq!(ws.open_graph().unwrap().2, graph);
        assert_eq!(graph.read(cx).edited_name(cx), "Unsaved definition");
    });
}
