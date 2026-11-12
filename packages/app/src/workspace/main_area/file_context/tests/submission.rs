use super::*;
use crate::ui::InputContent;

fn prepare(
    ws: &mut Workspace,
    file: PaneId,
    attached: bool,
    full: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> (PaneId, InputContent) {
    let target = super::composer::add_selected_context(ws, file, window, cx);
    ws.maybe_connect_agent_chat(target, cx);
    ws.agent_chat_view(target)
        .unwrap()
        .clone()
        .update(cx, |view, cx| {
            view.set_turn_in_flight();
            if full {
                view.fill_queue_for_test(daruda_chat::QUEUE_DEPTH_MAX);
            }
            if attached {
                view.add_attachments(
                    vec![daruda_acp::PromptAttachment {
                        name: "design.md".into(),
                        content: daruda_acp::AttachmentContent::ResourceLink {
                            uri: "urn:test:design".into(),
                        },
                    }],
                    cx,
                )
                .unwrap();
            }
        });
    ws.input_dock.input.update(cx, |state, cx| {
        state.set_selected_range(0..0, cx);
        state.insert("  Review ", window, cx);
        state.move_cursor_to_end(cx);
        state.insert("\n\n", window, cx);
    });
    let content = ws.input_dock.input.read(cx).content();
    ws.input_dock.drafts.insert(target, content.clone());
    (target, content)
}

#[gpui::test]
async fn accepted_submissions_clear_the_draft_and_restore_compact_context(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file, _) = fixture(cx);
    cx.update(Workspace::fake_agent_connections_for_test);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            for (case, attached) in [("plain", false), ("attached", true)] {
                let (target, content) = prepare(ws, file, attached, false, window, cx);
                let wire = prompt_text(&content, None);
                let trimmed = wire.trim_end_matches(['\n', '\r']);
                let expected = if attached { trimmed } else { trimmed.trim() };
                ws.send_terminal_input(window, cx);
                assert!(ws.input_dock.input.read(cx).value().is_empty(), "{case}");
                assert!(!ws.input_dock.drafts.contains_key(&target), "{case}");
                let view = ws.agent_chat_view(target).unwrap().read(cx);
                let (prompt, _) = view.queue().in_strip_order().next().unwrap();
                assert_eq!(prompt.text(), expected, "{case}");
                assert_eq!(prompt.attachments().len(), usize::from(attached), "{case}");
                assert!(view.draft_attachments().is_empty(), "{case}");
                let id = prompt.id();
                ws.begin_edit_queued_prompt(target, id, window, cx);
                assert_eq!(ws.input_dock.input.read(cx).content(), content, "{case}");
                ws.cancel_edit_queued_prompt(target, window, cx);
            }
        })
    })
    .unwrap();
}

#[gpui::test]
async fn refused_submissions_keep_the_draft_and_attachments(cx: &mut TestAppContext) {
    let (handle, workspace, _temp, file, _) = fixture(cx);
    cx.update(Workspace::fake_agent_connections_for_test);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |ws, cx| {
            for (case, attached) in [("plain", false), ("attached", true)] {
                let (target, content) = prepare(ws, file, attached, true, window, cx);
                ws.send_terminal_input(window, cx);
                assert_eq!(ws.input_dock.input.read(cx).content(), content, "{case}");
                assert_eq!(ws.input_dock.drafts.get(&target), Some(&content), "{case}");
                let view = ws.agent_chat_view(target).unwrap().read(cx);
                assert_eq!(
                    view.queue().in_strip_order().count(),
                    daruda_chat::QUEUE_DEPTH_MAX,
                    "{case}"
                );
                assert_eq!(
                    view.draft_attachments().len(),
                    usize::from(attached),
                    "{case}"
                );
            }
        })
    })
    .unwrap();
}
