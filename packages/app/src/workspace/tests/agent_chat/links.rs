//! Rendered link input through the chat host and file-viewer boundary.
use super::*;

#[gpui::test]
async fn rendered_chat_links_open_outside_files_as_read_only_references(cx: &mut TestAppContext) {
    use crate::workspace::main_area::file_view_pane::FileOrigin;
    use daruda_chat::view::AgentSessionStatus;
    use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, VisualTestContext};

    let documents = tempfile::tempdir().unwrap();
    let paths = [
        documents.path().join("plan.md"),
        documents.path().join("design with space.md"),
    ];
    for path in &paths {
        std::fs::write(path, "# Outside repository\n").unwrap();
    }
    let lane = tempfile::tempdir().unwrap();
    let (window_handle, workspace) = crate::workspace::tests::build_workspace_with(
        cx,
        &daruda_config::Config::default(),
        Some(daruda_store::project::Project::from_path(lane.path())),
    );
    cx.run_until_parked();
    let (pane, tab) = cx
        .update_window(window_handle.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let cwd = ws.active_lane().unwrap().path.clone();
                let pane = ws
                    .insert_agent_chat_pane(
                        daruda_config::AgentDefinition::claude_default().id,
                        (Some(cwd), None, None),
                        daruda_config::ModeChoice::FollowDefault,
                        window,
                        cx,
                    )
                    .unwrap();
                let view = ws.agent_chat_view(pane).unwrap().clone();
                view.update(cx, |view, cx| {
                    view.set_status_for_test(AgentSessionStatus::Connected);
                    view.seed_items_for_test(
                        [daruda_acp::ChatItem::AssistantText {
                            // A raw Windows path loses `\` before punctuation to
                            // Markdown escaping (`Temp\.tmp…`), so links carry URLs.
                            text: format!(
                                "- [First link](<{}>)\n- [Second link](<{}>)",
                                daruda_core::file_url::from_local_path(&paths[0]).unwrap(),
                                daruda_core::file_url::from_local_path(&paths[1]).unwrap()
                            ),
                            streaming: false,
                            message_id: None,
                            phase: Default::default(),
                        }],
                        cx,
                    );
                });
                ws.reveal_new_agent_chat_pane(pane, window, cx);
                (pane, ws.active_runtime().active_tab_index)
            })
        })
        .unwrap();
    let mut vcx = VisualTestContext::from_window(window_handle.into(), cx);
    for (label, path) in ["First link", "Second link"].into_iter().zip(paths) {
        vcx.update(|window, cx| workspace.update(cx, |ws, cx| ws.activate_tab(tab, window, cx)));
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
        let at = vcx.update(|window, _| {
            let bounds =
                crate::ui::test_support::painted_text_bounds(window, label).expect("link painted");
            // Paragraph bounds include trailing space; the first glyph is a link.
            gpui::point(bounds.left() + gpui::px(1.), bounds.center().y)
        });
        vcx.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Right,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert!(
                workspace.read(cx).main_area.popup_menu_deploy.is_some(),
                "link right press opens its menu"
            );
        });
        vcx.simulate_keystrokes("down enter");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(path.to_string_lossy().into_owned())
            );
        });
        vcx.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Right,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.run_until_parked();
        vcx.simulate_keystrokes("down down down down enter");
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            workspace.update(cx, |ws, cx| {
                let requests = ws.take_external_open_requests(cx);
                assert_eq!(
                    requests.len(),
                    1,
                    "external menu action records one request"
                );
                assert_eq!(requests[0].path, path);
            })
        });
        vcx.run_until_parked();
        vcx.simulate_event(MouseDownEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        vcx.simulate_event(MouseUpEvent {
            position: at,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            workspace.read_with(cx, |ws, cx| {
                let file = ws.focused_file_view().unwrap_or_else(|| {
                    panic!(
                        "link opens a file pane; errors={:?}; pane={pane:?}; tabs={}",
                        ws.error_history(),
                        ws.active_runtime().tabs.len()
                    )
                });
                assert_eq!(file.path, path);
                assert_eq!(file.origin, FileOrigin::Reference);
                assert!(!file.holds_editable_buffer());
                assert!(
                    ws.agent_chat_view(pane)
                        .unwrap()
                        .read(cx)
                        .any_handle()
                        .is_none()
                );
            });
        });
    }
}
