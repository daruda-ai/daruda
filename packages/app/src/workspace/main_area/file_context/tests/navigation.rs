use super::*;

#[gpui::test]
async fn opening_a_reference_restores_its_file_version_and_source_line(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file_id, owner) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            for source in [
                DiffSource::WorkingTree,
                DiffSource::Index,
                DiffSource::Range {
                    from: "base".into(),
                    to: "tip".into(),
                    old_path: Some("previous.rs".into()),
                    status: 'D',
                },
            ] {
                ws.set_focused_pane(file_id, window, cx);
                let file = ws.focused_file_content_mut().unwrap();
                file.view.source = source.clone();
                file.editor_state.update(cx, |state, cx| {
                    let start = SOURCE.find("    work();").unwrap();
                    state.set_selected_range(start..start + "    work();".len(), cx);
                });
                ws.add_file_reference(file_id, true, window, cx);
                let token = ws
                    .input_dock
                    .input
                    .read(cx)
                    .tokens()
                    .last()
                    .unwrap()
                    .token()
                    .clone();
                let tab = ws.tab_index_for_pane(owner).unwrap();
                ws.activate_tab(tab, window, cx);
                ws.open_input_reference(&token, window, cx);
                let file = ws.focused_file_content().unwrap();
                assert_eq!(file.view.source, source);
                assert_eq!(file.view.view_mode, FileViewMode::Raw);
                assert_eq!(
                    file.editor_state.read(cx).cursor(),
                    SOURCE.find("    work();").unwrap()
                );
                assert_eq!(ws.active_runtime().focused_pane_id, file_id);
            }
        })
    })
    .unwrap();
}

#[gpui::test]
async fn opening_a_diff_reference_restores_hidden_context_and_the_selected_row(
    cx: &mut TestAppContext,
) {
    use crate::workspace::main_area::file_view_pane::{build_diff_rows, parse_diff_hunks};

    let (handle, workspace, _temp, file_id, _) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let hunks = parse_diff_hunks("@@ -1,3 +1,3 @@\n before\n-old\n+new\n after\n");
            let file = ws.focused_file_content_mut().unwrap();
            file.view.source = DiffSource::Index;
            file.view.view_mode = FileViewMode::Changes;
            file.install_content(
                PaneFileContent::LoadedDiff {
                    rows_all: build_diff_rows(&hunks, false),
                    rows_no_ctx: build_diff_rows(&hunks, true),
                    added: 1,
                    removed: 1,
                },
                Vec::new(),
                Some(&mut *window),
                cx,
            );
            ws.toggle_hide_unchanged_for_pane(file_id, window, cx);
            ws.focused_file_content()
                .unwrap()
                .editor_state
                .clone()
                .update(cx, |state, cx| {
                    let start = state.value().find("new").unwrap();
                    state.set_selected_range(start..start + "new".len(), cx);
                });
            ws.add_file_reference(file_id, true, window, cx);
            let token = ws.input_dock.input.read(cx).tokens()[0].token().clone();
            ws.toggle_hide_unchanged_for_pane(file_id, window, cx);
            assert!(
                ws.focused_file_content()
                    .unwrap()
                    .editor_state
                    .read(cx)
                    .value()
                    .contains("before")
            );
            ws.open_input_reference(&token, window, cx);
            let file = ws.focused_file_content().unwrap();
            assert_eq!(file.view.source, DiffSource::Index);
            assert_eq!(file.view.view_mode, FileViewMode::Changes);
            assert!(file.view.hide_unchanged);
            let state = file.editor_state.read(cx);
            assert!(!state.value().contains("before"));
            assert_eq!(state.cursor(), state.value().find("new").unwrap());
        })
    })
    .unwrap();
}
