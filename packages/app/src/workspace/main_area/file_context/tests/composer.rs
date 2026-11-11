use super::*;
use daruda_acp::{AcpEvent, AcpSessionHandle, QuestionField, QuestionFieldKind, QuestionRequest};

fn add_selected_context(
    ws: &mut Workspace,
    file: PaneId,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> PaneId {
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
    let tab_id = ws.alloc_id();
    ws.active_runtime_mut().tabs.push(TabEntry {
        id: tab_id,
        layout: PaneLayout::Pane(target),
        last_focused_pane: target,
        user_label: None,
    });
    ws.select_bottom_input_pane(target, window, cx);
    ws.focused_file_content()
        .unwrap()
        .editor_state
        .clone()
        .update(cx, |state, cx| {
            let start = SOURCE.find("    work();").unwrap();
            state.set_selected_range(start..start + "    work();".len(), cx);
        });
    ws.add_file_reference(file, true, window, cx);
    ws.sync_question_composer(window, cx);
    target
}

fn ask(
    ws: &mut Workspace,
    target: PaneId,
    cx: &mut Context<Workspace>,
) -> daruda_acp::QuestionProbe {
    let (handle, _) = AcpSessionHandle::detached_for_test();
    let answer = handle.park_question_for_test(1);
    ws.agent_chat_view(target)
        .unwrap()
        .clone()
        .update(cx, |view, _| {
            view.attach_handle(handle);
            view.set_status_for_test(daruda_chat::view::AgentSessionStatus::Connected);
            view.set_turn_in_flight();
        });
    ws.fold_agent_chat_event(
        target,
        AcpEvent::QuestionAsked {
            id: 1,
            question: QuestionRequest {
                message: "Which implementation?".into(),
                tool_call_id: None,
                delivery: daruda_acp::QuestionDelivery::Blocking,
                fields: vec![QuestionField {
                    key: "implementation".into(),
                    title: None,
                    description: None,
                    required: false,
                    kind: QuestionFieldKind::Text,
                    companion_of: None,
                    default: None,
                }],
            },
        },
        cx,
    );
    answer
}

#[gpui::test]
async fn slash_completion_and_undo_keep_the_selection_snapshot(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file, _) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let target = add_selected_context(ws, file, window, cx);
            ws.fold_agent_chat_event(
                target,
                AcpEvent::AvailableCommandsChanged(vec![daruda_acp::SlashCommand {
                    name: "explain".into(),
                    description: "Explain the code".into(),
                    input: daruda_acp::SlashCommandInput::FreeText {
                        hint: "Question".into(),
                    },
                }]),
                cx,
            );
            ws.input_dock.input.update(cx, |state, cx| {
                state.set_selected_range(0..0, cx);
                state.insert("/ex ", window, cx);
                state.set_selected_range(3..3, cx);
            });
            ws.refresh_slash_menu(cx);
        })
    })
    .unwrap();
    let vcx = &mut VisualTestContext::from_window(handle, cx);
    vcx.run_until_parked();
    let before = workspace.read_with(vcx, |ws, cx| ws.input_dock.input.read(cx).content());
    vcx.update(|window, cx| workspace.update(cx, |ws, cx| ws.accept_slash_item(None, window, cx)));
    vcx.run_until_parked();
    workspace.read_with(vcx, |ws, cx| {
        let content = ws.input_dock.input.read(cx).content();
        assert!(content.text().starts_with("/explain "));
        assert_eq!(content.tokens()[0].token(), before.tokens()[0].token());
        assert!(prompt_text(&content, None).contains("    work();"));
    });
    vcx.update(|window, cx| window.dispatch_action(Box::new(crate::ui::InputUndo), cx));
    vcx.run_until_parked();
    workspace.read_with(vcx, |ws, cx| {
        assert_eq!(ws.input_dock.input.read(cx).content(), before)
    });
}

#[gpui::test]
async fn a_held_prompt_keeps_its_context_when_trailing_newlines_are_removed(
    cx: &mut TestAppContext,
) {
    let (handle, workspace, _temp, file, _) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let target = add_selected_context(ws, file, window, cx);
            let mut answer = ask(ws, target, cx);
            ws.sync_question_composer(window, cx);
            assert!(!ws.focused_question_bound(cx));
            let before = ws.input_dock.input.read(cx).content();
            ws.input_dock.input.update(cx, |state, cx| {
                state.insert("\n\n", window, cx);
            });
            ws.send_terminal_input(window, cx);
            assert_eq!(ws.input_dock.input.read(cx).content(), before);
            assert!(
                ws.agent_chat_view(target)
                    .unwrap()
                    .read(cx)
                    .queue()
                    .in_strip_order()
                    .next()
                    .is_none()
            );
            ws.keep_prompt_queued_behind_question(target, window, cx);
            let view = ws.agent_chat_view(target).unwrap().read(cx);
            let (prompt, _) = view.queue().in_strip_order().next().unwrap();
            assert!(prompt.text().contains("    work();"));
            assert!(answer.answer().is_none());
        })
    })
    .unwrap();
}

#[gpui::test]
async fn file_context_cannot_be_bound_or_sent_as_a_question_answer(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file, _) = fixture(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            let target = add_selected_context(ws, file, window, cx);
            let mut answer = ask(ws, target, cx);
            let before = ws.input_dock.input.read(cx).content();
            ws.answer_question_from_strip(target, window, cx);
            assert!(!ws.focused_question_bound(cx));
            assert_eq!(ws.input_dock.input.read(cx).content(), before);
            // A restored or pasted reference in an already-bound answer is refused too.
            let view = ws.agent_chat_view(target).unwrap().clone();
            view.update(cx, |view, cx| {
                view.bind_question_composer(1, None, false, cx)
            });
            assert!(ws.focused_question_bound(cx));
            ws.send_terminal_input(window, cx);
            ws.submit_question_with_composer_text(target, 1, window, cx);
            ws.question_key(
                crate::workspace::main_area::bottom_dock::question_keys_ops::QuestionKey::NextField,
                window,
                cx,
            );
            assert_eq!(ws.input_dock.input.read(cx).content(), before);
            assert!(answer.answer().is_none());
        })
    })
    .unwrap();
}
