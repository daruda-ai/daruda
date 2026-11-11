use super::super::{
    file_view_pane::{DiffSource, FileOrigin},
    pane::{PaneContent, TabEntry},
    pane_tree::PaneLayout,
};
mod composer;
mod navigation;
use super::*;
use crate::workspace::tests::build_workspace_with;
use gpui::{
    AnyWindowHandle, AppContext as _, Entity, MouseButton, TestAppContext, VisualTestContext,
};

const SOURCE: &str = "fn f() {\n    work();\n}\n";

#[gpui::test]
async fn a_saved_selection_keeps_its_contents_after_source_edits_and_submission(
    cx: &mut TestAppContext,
) {
    let (handle, workspace, _temp, file_id, other) = fixture(cx);
    cx.update(Workspace::fake_agent_connections_for_test);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let root = ws.active_lane().unwrap().path.clone();
            let pane = ws.create_agent_chat_pane(
                Some(daruda_store::project::PaneCwd::Local(root)),
                None,
                daruda_config::AgentDefinition::claude_default().id,
                None,
                window,
                cx,
            );
            let target = pane.id;
            ws.active_runtime_mut().panes.push(pane);
            ws.maybe_connect_agent_chat(target, cx);
            ws.agent_chat_view(target)
                .unwrap()
                .clone()
                .update(cx, |view, _| view.set_turn_in_flight());
            ws.select_bottom_input_pane(target, window, cx);
            ws.input_dock.input.update(cx, |state, cx| {
                state.set_value("Please review: ", window, cx);
                state.move_cursor_to_end(cx);
            });
            let file = ws.focused_file_content().unwrap();
            let path = file.view.path.clone();
            let editor = file.editor_state.clone();
            std::fs::write(&path, SOURCE).unwrap();
            editor.update(cx, |state, cx| {
                let start = SOURCE.find("    work();").unwrap();
                state.set_selected_range(start..start + "    work();".len(), cx);
            });
            ws.add_file_reference(file_id, true, window, cx);
            let draft = ws.input_dock.input.read(cx).content();
            assert!(draft.text().starts_with("Please review: "));
            assert!(draft.text().contains("review.rs:2"));
            assert!(!draft.text().contains("work();"));
            assert_eq!(draft.tokens().len(), 1);
            let wire = prompt_text(&draft, None);
            assert!(wire.contains("    work();"));
            assert!(wire.contains(&s::file_viewer::input_selection_snapshot()));

            ws.select_bottom_input_pane(other, window, cx);
            ws.select_bottom_input_pane(target, window, cx);
            assert_eq!(ws.input_dock.input.read(cx).content(), draft);

            let changed = "fn f() {\n    changed();\n}\n";
            std::fs::write(&path, changed).unwrap();
            editor.update(cx, |state, cx| state.set_value(changed, window, cx));
            assert_eq!(ws.input_dock.input.read(cx).content(), draft);
            ws.send_terminal_input(window, cx);
            let view = ws.agent_chat_view(target).unwrap().read(cx);
            let (queued, _) = view.queue().in_strip_order().next().unwrap();
            assert_eq!(queued.text(), wire.trim());
            assert!(!queued.text().contains("changed();"));
            let queued_id = queued.id();
            assert_eq!(ws.active_runtime().focused_pane_id, file_id);

            ws.begin_edit_queued_prompt(target, queued_id, window, cx);
            assert_eq!(ws.input_dock.input.read(cx).content(), draft);
            ws.input_dock.input.update(cx, |state, cx| {
                state.insert("Explain the failure.", window, cx);
            });
            let edited = ws.input_dock.input.read(cx).content();
            ws.send_terminal_input(window, cx);
            let queue = ws.agent_chat_view(target).unwrap().read(cx).queue();
            assert_eq!(queue.in_strip_order().count(), 1);
            assert!(
                queue
                    .find(queued_id)
                    .unwrap()
                    .text()
                    .contains("Explain the failure.")
            );
            assert_eq!(
                ws.queued_composer_content(
                    target,
                    queued_id,
                    queue.find(queued_id).unwrap().text()
                ),
                edited
            );

            ws.clear_queued_prompts(target, window, cx);
            ws.do_history_navigate(
                crate::workspace::main_area::bottom_dock::composer_ops::HistoryDir::Up,
                window,
                cx,
            );
            assert_eq!(ws.input_dock.input.read(cx).content(), edited);
            ws.input_dock.input.update(cx, |state, cx| {
                let token = state.tokens()[0].range();
                state.set_selected_range(token, cx);
                state.replace("", window, cx);
            });
            let removed = ws.input_dock.input.read(cx).content();
            assert!(removed.tokens().is_empty());
            assert!(!prompt_text(&removed, None).contains("work();"));
        });
    })
    .unwrap();
}

#[gpui::test]
async fn send_actions_follow_the_selected_input_agents_name(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file_id, _) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let agents = [
                daruda_config::AgentDefinition::claude_default(),
                daruda_config::AgentDefinition::codex_default(),
            ];
            let mut config = daruda_config::Config::default();
            config
                .agents
                .push(daruda_config::AgentEntry::for_definition(
                    agents[1].clone(),
                    None,
                ));
            ws.apply_config(&config, cx);
            for agent in agents {
                let root = ws.active_lane().unwrap().path.clone();
                let pane = ws.create_agent_chat_pane(
                    Some(daruda_store::project::PaneCwd::Local(root)),
                    None,
                    agent.id,
                    Some("A conversation title".into()),
                    window,
                    cx,
                );
                let id = pane.id;
                ws.active_runtime_mut().panes.push(pane);
                ws.select_bottom_input_pane(id, window, cx);
                let action = ws.file_send_action();
                assert_eq!(action.label, s::file_viewer::send_to(&agent.name));
                assert!(action.tooltip.contains(&agent.name));
                assert_eq!(ws.active_runtime().focused_pane_id, file_id);
            }
            ws.input_dock.owner = None;
            assert_eq!(ws.file_send_action().label, s::file_viewer::send_to_agent());
        });
    })
    .unwrap();
}

#[gpui::test]
async fn the_busy_file_recipient_button_queues_instead_of_stopping(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file_id, _) = fixture(cx);
    cx.update(Workspace::fake_agent_connections_for_test);
    let target = cx
        .update_window(handle, |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let root = ws.active_lane().unwrap().path.clone();
                let chat = ws.create_agent_chat_pane(
                    Some(daruda_store::project::PaneCwd::Local(root)),
                    None,
                    daruda_config::AgentDefinition::claude_default().id,
                    None,
                    window,
                    cx,
                );
                let id = chat.id;
                ws.active_runtime_mut().panes.push(chat);
                let tab_id = ws.alloc_id();
                ws.active_runtime_mut().tabs.push(TabEntry {
                    id: tab_id,
                    layout: PaneLayout::Pane(id),
                    last_focused_pane: id,
                    user_label: None,
                });
                ws.maybe_connect_agent_chat(id, cx);
                ws.agent_chat_view(id)
                    .unwrap()
                    .clone()
                    .update(cx, |view, _| view.set_turn_in_flight());
                ws.select_bottom_input_pane(id, window, cx);
                ws.input_dock
                    .input
                    .update(cx, |state, cx| state.set_value("Follow-up", window, cx));
                ws.docks.bottom.update(cx, |dock, _| dock.open());
                id
            })
        })
        .unwrap();
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let send = vcx
        .debug_bounds("composer-submit")
        .expect("primary input action renders");
    vcx.simulate_click(send.center(), Default::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |ws, cx| {
        let view = ws.agent_chat_view(target).unwrap().read(cx);
        assert_eq!(view.queue().in_strip_order().count(), 1);
        assert!(ws.input_dock.input.read(cx).value().is_empty());
        assert_eq!(ws.active_runtime().focused_pane_id, file_id);
    });
}

fn fixture(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<Workspace>,
    tempfile::TempDir,
    PaneId,
    PaneId,
) {
    let temp = tempfile::tempdir().unwrap();
    let project = daruda_store::project::Project::from_path(temp.path());
    let (handle, workspace) =
        build_workspace_with(cx, &daruda_config::Config::default(), Some(project));
    cx.run_until_parked();
    let (id, owner) = cx
        .update_window(handle.into(), |_, window, cx| {
            workspace.update(cx, |ws, cx| {
                let owner = ws.active_runtime().focused_pane_id;
                ws.bind_bottom_input(owner, window, cx);
                ws.input_dock.input.update(cx, |state, cx| {
                    state.set_value("Review this: ", window, cx);
                    state.move_cursor_to_end(cx);
                });
                let root = ws.active_lane().expect("lane").path.clone();
                let mut file = ws.create_file_pane(
                    FileOrigin::Lane,
                    root.join("review.rs"),
                    DiffSource::WorkingTree,
                    None,
                    FileViewMode::Raw,
                    window,
                    cx,
                );
                let id = file.id;
                let content = file.file_content_mut().unwrap();
                content.install_content(
                    PaneFileContent::LoadedRaw { truncated: false },
                    Vec::new(),
                    Some(&mut *window),
                    cx,
                );
                content.saved_text = SOURCE.into();
                content
                    .editor_state
                    .update(cx, |state, cx| state.set_value(SOURCE, window, cx));
                ws.active_runtime_mut().panes.push(file);
                let tab_id = ws.alloc_id();
                ws.active_runtime_mut().tabs.push(TabEntry {
                    id: tab_id,
                    layout: PaneLayout::Pane(id),
                    last_focused_pane: id,
                    user_label: None,
                });
                ws.active_runtime_mut().active_tab_index = ws.active_runtime().tabs.len() - 1;
                ws.set_focused_pane(id, window, cx);
                ws.activate_bottom_input(cx);
                (id, owner)
            })
        })
        .unwrap();
    (handle.into(), workspace, temp, id, owner)
}

#[gpui::test]
async fn adding_a_reference_keeps_the_file_and_input_owner(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, id, owner) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            ws.add_file_reference(id, false, window, cx);
            assert_eq!(ws.active_runtime().focused_pane_id, id);
            assert_eq!(ws.bottom_input_pane(), Some(owner));
            assert!(
                ws.input_dock
                    .input
                    .read(cx)
                    .value()
                    .starts_with("Review this: ")
            );
            assert!(ws.input_dock.input.read(cx).value().contains("review.rs"));
            let before = ws.input_dock.input.read(cx).value().to_string();
            ws.input_dock.owner = None;
            ws.add_file_reference(id, true, window, cx);
            assert_eq!(ws.input_dock.input.read(cx).value(), before);
        })
    })
    .unwrap();
}

#[gpui::test]
async fn real_file_selection_and_right_click_reach_the_input(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, id, owner) = fixture(cx);
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let (start, end) = workspace.read_with(vcx, |ws, cx| {
        let editor = ws.focused_file_content().unwrap().editor_state.read(cx);
        (
            editor.range_to_bounds(&(0..1)).unwrap().center(),
            editor.range_to_bounds(&(17..18)).unwrap().center(),
        )
    });
    vcx.simulate_mouse_down(start, MouseButton::Left, Default::default());
    vcx.simulate_mouse_move(end, Some(MouseButton::Left), Default::default());
    vcx.simulate_mouse_up(end, MouseButton::Left, Default::default());
    vcx.run_until_parked();
    assert!(
        workspace.read_with(vcx, |ws, _| ws
            .focused_file_content()
            .unwrap()
            .selection_action
            .is_some()),
        "release shows the selection action"
    );
    let button = vcx
        .debug_bounds("file-selection-add")
        .expect("selection action is painted at the workspace root");
    vcx.simulate_click(button.center(), Default::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |ws, cx| {
        assert_eq!(ws.active_runtime().focused_pane_id, id);
        assert_eq!(ws.bottom_input_pane(), Some(owner));
        assert!(
            ws.input_dock
                .input
                .read(cx)
                .value()
                .contains("review.rs:1-2")
        );
    });
    vcx.update(|window, cx| {
        workspace.read_with(cx, |ws, cx| {
            let editor = ws.focused_file_content().unwrap().editor_state.read(cx);
            assert!(!editor.selected_range().is_empty());
            assert!(editor.has_selection_focus(window, cx));
        });
    });
    vcx.simulate_mouse_down(start, MouseButton::Right, Default::default());
    vcx.run_until_parked();
    assert!(
        workspace.read_with(vcx, |ws, _| ws.main_area.popup_menu_deploy.is_some()),
        "the editor's right press opens the host menu"
    );
    vcx.update(|window, cx| {
        workspace.read_with(cx, |ws, cx| {
            let editor = ws.focused_file_content().unwrap().editor_state.read(cx);
            assert!(editor.has_selection_focus(window, cx));
        });
    });
}

#[gpui::test]
async fn an_unowned_input_can_select_its_only_remaining_recipient(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file, owner) = fixture(cx);
    cx.update_window(handle, |_, _, cx| {
        workspace.update(cx, |ws, cx| {
            ws.input_dock.owner = None;
            ws.docks.bottom.update(cx, |dock, _| dock.open());
            cx.notify();
        });
    })
    .unwrap();
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let choose = vcx.debug_bounds("input-select-recipient").unwrap();
    vcx.simulate_click(choose.center(), Default::default());
    vcx.run_until_parked();
    workspace.read_with(vcx, |ws, _| {
        assert_eq!(ws.bottom_input_pane(), Some(owner));
        assert_eq!(ws.active_runtime().focused_pane_id, file);
    });
}

#[gpui::test]
async fn a_file_target_connects_only_when_the_person_submits(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, id, _) = fixture(cx);
    let probes = cx.update(Workspace::fake_agent_connections_for_test);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let root = ws.active_lane().unwrap().path.clone();
            let chat = ws.create_agent_chat_pane(
                Some(daruda_store::project::PaneCwd::Local(root)),
                None,
                daruda_config::AgentDefinition::claude_default().id,
                None,
                window,
                cx,
            );
            let target = chat.id;
            ws.active_runtime_mut().panes.push(chat);
            ws.select_bottom_input_pane(target, window, cx);
            ws.add_file_reference(id, false, window, cx);
            assert!(
                probes.borrow().is_empty(),
                "selection and reference insertion do not connect"
            );
            ws.send_terminal_input(window, cx);
            assert_eq!(
                probes.borrow().len(),
                1,
                "explicit submit lazily connects the selected recipient"
            );
            assert_eq!(ws.active_runtime().focused_pane_id, id);
            ws.input_dock
                .input
                .update(cx, |state, cx| state.set_value("/clear", window, cx));
            ws.send_terminal_input(window, cx);
            assert!(
                ws.input_dock.input.read(cx).value().is_empty(),
                "a locally handled command consumes the draft"
            );
            let Some(PaneContent::AgentChat(_)) = ws
                .active_runtime()
                .panes
                .iter()
                .find(|p| p.id == target)
                .map(|p| &p.content)
            else {
                panic!("chat target");
            };
        })
    })
    .unwrap();
}
