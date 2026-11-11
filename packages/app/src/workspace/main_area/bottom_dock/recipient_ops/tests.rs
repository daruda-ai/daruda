use crate::workspace::tests::build_workspace_with;
use gpui::{AppContext as _, TestAppContext};

#[gpui::test]
async fn file_review_input_switch_preserves_the_file_and_each_draft(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let project = daruda_store::project::Project::from_path(temp.path());
    let (handle, workspace) =
        build_workspace_with(cx, &daruda_config::Config::default(), Some(project));
    cx.run_until_parked();
    let (original, file_id) = cx
        .update_window(handle.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let original = ws.active_runtime().focused_pane_id;
                ws.bind_bottom_input(original, window, cx);
                ws.input_dock.input.update(cx, |state, cx| {
                    state.set_value("original draft", window, cx)
                });
                let other = ws.create_pane(window, cx).expect("second input pane");
                let other_id = other.id;
                ws.active_runtime_mut().panes.push(other);
                let tab_id = ws.alloc_id();
                ws.active_runtime_mut()
                    .tabs
                    .push(crate::workspace::main_area::pane::TabEntry {
                        id: tab_id,
                        layout: crate::workspace::main_area::pane_tree::PaneLayout::Pane(other_id),
                        last_focused_pane: other_id,
                        user_label: None,
                    });
                let root = ws.active_lane().expect("lane").path.clone();
                let file = ws.create_file_pane(
                    crate::workspace::main_area::file_view_pane::FileOrigin::Lane,
                    root.join("review.rs"),
                    crate::workspace::main_area::file_view_pane::DiffSource::WorkingTree,
                    None,
                    crate::workspace::main_area::file_view_pane::FileViewMode::Raw,
                    window,
                    cx,
                );
                let file_id = file.id;
                ws.active_runtime_mut().panes.push(file);
                let tab_id = ws.alloc_id();
                ws.active_runtime_mut()
                    .tabs
                    .push(crate::workspace::main_area::pane::TabEntry {
                        id: tab_id,
                        layout: crate::workspace::main_area::pane_tree::PaneLayout::Pane(file_id),
                        last_focused_pane: file_id,
                        user_label: None,
                    });
                ws.active_runtime_mut().active_tab_index = ws.active_runtime().tabs.len() - 1;
                ws.docks.bottom.update(cx, |dock, _| dock.open());
                ws.set_focused_pane(file_id, window, cx);
                ws.select_bottom_input_pane(other_id, window, cx);
                assert_eq!(ws.active_runtime().focused_pane_id, file_id);
                assert_eq!(ws.bottom_input_pane(), Some(other_id));
                ws.input_dock
                    .input
                    .update(cx, |state, cx| state.set_value("other draft", window, cx));
                ws.select_bottom_input_pane(original, window, cx);
                assert_eq!(ws.input_dock.input.read(cx).value(), "original draft");
                ws.select_bottom_input_pane(other_id, window, cx);
                assert_eq!(ws.input_dock.input.read(cx).value(), "other draft");
                (original, file_id)
            })
        })
        .unwrap();
    let vcx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let selector: &'static str = Box::leak(format!("input-recipient-{original}").into_boxed_str());
    let tab = vcx
        .debug_bounds(selector)
        .expect("input recipient tab renders");
    vcx.simulate_click(tab.center(), Default::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |ws, cx| {
        assert_eq!(ws.bottom_input_pane(), Some(original));
        assert_eq!(ws.active_runtime().focused_pane_id, file_id);
        assert_eq!(ws.input_dock.input.read(cx).value(), "original draft");
    });
}
