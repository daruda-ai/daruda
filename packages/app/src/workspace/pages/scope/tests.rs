use super::*;

#[test]
fn shared_scope_resolves_without_changing_the_current_project() {
    let a = ProjectUuid::new();
    let b = ProjectUuid::new();
    for (label, scope, matches_a, matches_b) in [
        ("current", ProjectScope::Current, true, false),
        ("all", ProjectScope::All, true, true),
        ("pinned", ProjectScope::Project(b), false, true),
    ] {
        assert_eq!(scope.matches(a, Some(a)), matches_a, "{label}");
        assert_eq!(scope.matches(b, Some(a)), matches_b, "{label}");
        assert_eq!(ProjectScope::from(TaskScope::from(scope)), scope);
    }
}

use gpui::{TestAppContext, VisualTestContext};

fn two_projects(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    gpui::WindowHandle<gpui_component::Root>,
    gpui::Entity<Workspace>,
    ProjectId,
    ProjectId,
) {
    let dir = tempfile::tempdir().unwrap();
    let (window, ws) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(dir.path())),
    );
    let (first, second) = ws.update(cx, |ws, _| {
        let first = ws.active.project;
        let second = ws.next_project_id;
        ws.next_project_id += 1;
        let project =
            crate::project::Project::bootstrap_placeholder(second, dir.path().join("other"));
        let lane = daruda_store::project::LaneRef {
            project: second,
            lane: project.lanes[0].id,
        };
        ws.projects.push(project);
        ws.main_area.runtimes.entry(lane).or_default();
        (first, second)
    });
    (dir, window, ws, first, second)
}

#[gpui::test]
async fn both_docks_select_the_browsed_project_and_only_the_chevron_folds(cx: &mut TestAppContext) {
    let (_dir, window, ws, first, second) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    for expanded in [true, false] {
        ws.update_in(&mut vcx, |ws, window, cx| {
            ws.docks.left.update(cx, |dock, cx| {
                dock.is_open = expanded;
                cx.notify();
            });
            ws.open_page(Page::Tasks, window, cx);
            ws.select_page_scope(ProjectScope::All, window, cx);
        });
        vcx.run_until_parked();
        let prefix = if expanded {
            "project-header"
        } else {
            "rail-project"
        };
        let select = Box::leak(format!("{prefix}-{second}").into_boxed_str());
        let bounds = vcx.debug_bounds(select).expect("project selection");
        // The name/initial is separate from the trailing fold target.
        vcx.simulate_click(
            gpui::point(
                bounds.left() + gpui::px(crate::ui::theme::GAP_SM),
                bounds.center().y,
            ),
            Default::default(),
        );
        vcx.run_until_parked();
        ws.read_with(&vcx, |ws, _| {
            assert_eq!(ws.page_project(), Some(second));
            assert_eq!(ws.active_page(), Some(Page::Tasks));
            assert_eq!(ws.active.project, first);
        });
        let prefix = if expanded {
            "project-fold"
        } else {
            "rail-project-fold"
        };
        let fold = Box::leak(format!("{prefix}-{second}").into_boxed_str());
        for collapsed in [true, false] {
            let bounds = vcx.debug_bounds(fold).expect("separate project disclosure");
            vcx.simulate_click(bounds.center(), Default::default());
            vcx.run_until_parked();
            ws.read_with(&vcx, |ws, _| {
                assert_eq!(ws.project_for(second).unwrap().is_collapsed, collapsed);
                assert_eq!(ws.page_project(), Some(second));
                assert_eq!(ws.active_page(), Some(Page::Tasks));
                assert_eq!(ws.active.project, first);
            });
        }
    }
}

#[gpui::test]
fn a_visible_dirty_detail_guards_project_switch_with_cancel_and_discard(cx: &mut TestAppContext) {
    let (_dir, window, ws, first, second) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let editor = ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_task_editor(None, window, cx);
        let editor = ws.task_detail_id().unwrap();
        ws.task_editor(editor)
            .unwrap()
            .title_input
            .clone()
            .update(cx, |input, cx| {
                input.set_value("Unsaved navigation", window, cx)
            });
        ws.select_project(second, window, cx);
        editor
    });
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(&crate::surface::strings::common::btn_cancel());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.page_project(), Some(first));
        assert_eq!(ws.task_detail_id(), Some(editor));
    });
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.select_project(second, window, cx)
    });
    assert!(vcx.has_pending_prompt());
    vcx.simulate_prompt_answer(&crate::surface::strings::task::edit_discard());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.page_project(), Some(second));
        assert_eq!(ws.task_detail_id(), None);
        assert_eq!(ws.active.project, first);
        assert_eq!(ws.active_page(), Some(Page::Tasks));
    });
}

#[gpui::test]
fn hidden_drafts_wait_for_their_scope_and_worktree_return_preserves_them(cx: &mut TestAppContext) {
    let (_dir, window, ws, first, second) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_task_editor(None, window, cx);
        let editor = ws.task_detail_id().unwrap();
        ws.task_editor(editor)
            .unwrap()
            .title_input
            .clone()
            .update(cx, |input, cx| input.set_value("Kept draft", window, cx));
        ws.open_page(Page::Flows, window, cx);
        ws.select_project(second, window, cx);
        assert_eq!(
            ws.flow_browser_lanes()
                .iter()
                .map(|lane| lane.project)
                .collect::<Vec<_>>(),
            vec![second]
        );
        ws.open_page(Page::PullRequests, window, cx);
        assert_eq!(ws.page_project(), Some(second));
        ws.open_page(Page::Tasks, window, cx);
        assert_eq!(
            ws.shown_page_detail_id(),
            None,
            "another project's draft stays hidden"
        );
        assert!(
            !ws.close_page_detail(window, cx),
            "Escape cannot discard a hidden draft"
        );
        assert_eq!(ws.task_detail_id(), Some(editor));
        ws.select_project(first, window, cx);
        assert_eq!(ws.shown_page_detail_id(), Some(editor.into()));
        ws.return_to_worktree(window, cx);
        assert_eq!(ws.task_detail_id(), Some(editor));
        let target = ws.projects.get(second).unwrap();
        let lane = daruda_store::project::LaneRef {
            project: second,
            lane: target.lanes[0].id,
        };
        ws.activate_lane(lane, window, cx);
        ws.open_page(Page::Tasks, window, cx);
        assert_eq!(
            ws.page_project(),
            Some(first),
            "the browsing scope is pinned across runtime switches"
        );
        assert_eq!(ws.shown_page_detail_id(), Some(editor.into()));
        assert_eq!(
            ws.task_editor(editor).unwrap().title_input.read(cx).value(),
            "Kept draft"
        );
    });
    assert!(
        !vcx.has_pending_prompt(),
        "hiding is reversible and leaves the draft intact"
    );
}

#[gpui::test]
fn page_scroll_positions_survive_page_changes_and_worktree_return(cx: &mut TestAppContext) {
    let (_dir, window, ws, _, _) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        for (index, page) in Page::ALL.into_iter().enumerate() {
            ws.open_page(page, window, cx);
            ws.workspace_page
                .state()
                .unwrap()
                .scroll
                .set_offset(gpui::point(
                    gpui::px(0.),
                    gpui::px(-40. * (index + 1) as f32),
                ));
        }
        ws.return_to_worktree(window, cx);
        for (index, page) in Page::ALL.into_iter().enumerate() {
            ws.open_page(page, window, cx);
            assert_eq!(
                ws.workspace_page.state().unwrap().scroll.offset().y,
                gpui::px(-40. * (index + 1) as f32),
                "{page:?}"
            );
        }
    });
}

#[gpui::test]
fn restoring_the_shown_detail_recovers_its_project_without_switching_runtime(
    cx: &mut TestAppContext,
) {
    let (_dir, window, ws, first, second) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        let project = ws.project_for(second).unwrap().uuid;
        ws.open_task_draft_for_project(project, window, cx);
        let (state, projects) = ws.snapshot_for_disk(cx);
        ws.set_page_scope(
            ProjectScope::Project(ws.project_for(first).unwrap().uuid),
            cx,
        );
        ws.restore_from_disk(&state, &projects, window, cx);
        assert_eq!(ws.active.project, first);
        assert_eq!(ws.page_project(), Some(second));
        assert!(ws.shown_page_detail_id().is_some());
    });
}

#[gpui::test]
fn task_action_uses_browsed_project_and_all_scope_waits_for_a_choice(cx: &mut TestAppContext) {
    let (_dir, window, ws, first, second) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Tasks, window, cx);
        ws.select_project(second, window, cx);
        ws.on_new_task(&crate::workspace::NewTask, window, cx);
        let editor = ws.task_detail_id().unwrap();
        assert_eq!(
            ws.task_editor(editor).unwrap().project(cx),
            Some(ws.project_for(second).unwrap().uuid)
        );
        assert_eq!(ws.active.project, first);
        ws.close_page_detail_now(editor.into(), cx);
        ws.select_page_scope(ProjectScope::All, window, cx);
        ws.on_new_task(&crate::workspace::NewTask, window, cx);
        assert_eq!(
            ws.task_detail_id(),
            None,
            "the project choice precedes a draft"
        );
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, _| {
        assert_eq!(ws.task_detail_id(), None);
        assert_eq!(ws.page_scope(), ProjectScope::All);
    });
    let button = vcx
        .debug_bounds("task-new")
        .expect("the header's New Task action");
    vcx.simulate_click(button.center(), Default::default());
    vcx.run_until_parked();
    let options = vcx
        .debug_bounds("task-project-options")
        .expect("the common project chooser");
    assert!(
        options.size.height > gpui::px(crate::ui::theme::CONTROL_TARGET_SIZE * 2.),
        "the query and both project rows must have painted height: {options:?}"
    );
    vcx.simulate_keystrokes("down enter");
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, cx| {
        let editor = ws
            .task_detail_id()
            .expect("confirming a project opens the draft");
        assert_eq!(
            ws.task_editor(editor).unwrap().project(cx),
            ws.page_scope().project(None)
        );
        assert_eq!(ws.active.project, first);
    });
}

#[gpui::test]
fn runtime_continue_button_restores_the_retained_draft_and_scope(cx: &mut TestAppContext) {
    let (_dir, window, ws, first, second) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let editor = ws.update_in(&mut vcx, |ws, window, cx| {
        ws.open_page(Page::Tasks, window, cx);
        ws.on_new_task(&crate::workspace::NewTask, window, cx);
        let editor = ws.task_detail_id().unwrap();
        ws.task_editor(editor)
            .unwrap()
            .title_input
            .clone()
            .update(cx, |input, cx| {
                input.set_value("Continue this draft", window, cx)
            });
        let project = ws.project_for(second).unwrap();
        let lane = daruda_store::project::LaneRef {
            project: second,
            lane: project.lanes[0].id,
        };
        ws.activate_lane(lane, window, cx);
        assert_eq!(ws.active_page(), None);
        editor
    });
    vcx.run_until_parked();
    let bounds = vcx
        .debug_bounds("resume-workspace-page")
        .expect("continue action in runtime");
    vcx.simulate_click(bounds.center(), Default::default());
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, cx| {
        assert_eq!(ws.active_page(), Some(Page::Tasks));
        assert_eq!(ws.page_project(), Some(first));
        assert_eq!(ws.active.project, second);
        assert_eq!(ws.task_detail_id(), Some(editor));
        assert_eq!(
            ws.task_editor(editor).unwrap().title_input.read(cx).value(),
            "Continue this draft"
        );
    });
    assert!(!vcx.has_pending_prompt());
}

#[gpui::test]
fn rehoming_a_hidden_draft_keeps_the_browsed_project(cx: &mut TestAppContext) {
    let (dir, window, ws, closing, survivor) = two_projects(cx);
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    ws.update_in(&mut vcx, |ws, window, cx| {
        let browsed = ws.next_project_id;
        ws.next_project_id += 1;
        ws.projects
            .push(crate::project::Project::bootstrap_placeholder(
                browsed,
                dir.path().join("browsed"),
            ));
        let uuid = ws.project_for(closing).unwrap().uuid;
        ws.open_task_draft_for_project(uuid, window, cx);
        let editor = ws.task_detail_id().unwrap();
        ws.task_editor(editor)
            .unwrap()
            .title_input
            .clone()
            .update(cx, |input, cx| input.set_value("Keep my draft", window, cx));
        ws.open_page(Page::Flows, window, cx);
        ws.select_project(browsed, window, cx);
        let scope = ws.page_scope();
        ws.close_active_project(window, cx);
        assert_eq!(ws.active.project, survivor);
        assert_eq!(
            ws.page_scope(),
            scope,
            "draft repair cannot replace another page's scope"
        );
        assert_eq!(
            ws.task_editor(editor).unwrap().project(cx),
            Some(ws.project_for(survivor).unwrap().uuid)
        );
        assert_eq!(
            ws.pages.tasks.detail.as_ref().unwrap().project,
            ws.task_editor(editor).unwrap().project(cx)
        );
        ws.on_resume_workspace_page(&crate::workspace::ResumeWorkspacePage, window, cx);
        assert_eq!(ws.active_page(), Some(Page::Flows));
        assert_eq!(ws.page_project(), Some(browsed));
        assert_eq!(
            ws.task_editor(editor).unwrap().title_input.read(cx).value(),
            "Keep my draft"
        );
    });
    assert!(!vcx.has_pending_prompt());
}
