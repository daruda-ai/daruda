use super::*;
use crate::test_support::{ControlFixture, workspace_with_agent_chat};
use crate::ui::InlineToken;
use daruda_acp::{AcpEvent, AcpSessionHandle};
use daruda_chat::view::{AgentChatView, AgentSessionStatus, PromptDispatch};
use gpui::{AppContext as _, Entity, TestAppContext};

const PROMPT_TEXT: &str = "review captured source";

fn content() -> InputContent {
    InputContent::new("review file.rs:2")
        .with_token(7..16, InlineToken::new("snapshot", "file.rs:2"))
        .unwrap()
}

fn cache_prompt(
    fixture: &ControlFixture,
    pane: PaneId,
    cx: &mut TestAppContext,
) -> (Entity<AgentChatView>, PromptId) {
    let view = fixture
        .workspace
        .read_with(cx, |ws, _| ws.agent_chat_view(pane).unwrap().clone());
    let id = view.update(cx, |view, cx| {
        view.detach_handle();
        view.set_status_for_test(AgentSessionStatus::Idle);
        view.set_turn_in_flight();
        assert!(matches!(
            view.send_prompt_text(PROMPT_TEXT.into(), cx),
            PromptDispatch::Queued
        ));
        view.queue().last_prompt_id().unwrap()
    });
    fixture.workspace.update(cx, |ws, _| {
        let lane = ws.lane_ref_for_pane(pane).unwrap();
        ws.lane_scoped_mut(lane)
            .queued_input_cache
            .remember(pane, id, PROMPT_TEXT, &content());
    });
    cx.run_until_parked();
    (view, id)
}

#[gpui::test]
fn cache_validates_payloads_and_keeps_equal_prompts_independent(cx: &mut TestAppContext) {
    let fixture = workspace_with_agent_chat(cx);
    let pane = fixture.pane();
    let (view, first) = cache_prompt(&fixture, pane, cx);
    let second = view.update(cx, |view, cx| {
        view.send_prompt_text(PROMPT_TEXT.into(), cx);
        view.queue().last_prompt_id().unwrap()
    });
    assert_ne!(first, second);
    let mut cache = QueuedInputCache::default();
    for id in [first, second] {
        cache.remember(pane, id, PROMPT_TEXT, &content());
    }
    cache.remember(pane + 1, first, PROMPT_TEXT, &content());
    assert_eq!(cache.content(pane, first, PROMPT_TEXT), Some(&content()));
    assert!(cache.content(pane, first, "changed source").is_none());

    // Replacing one equal prompt with plain text must not inherit its token
    // or remove the other prompt's independent presentation.
    cache.remember(pane, first, PROMPT_TEXT, &InputContent::new(PROMPT_TEXT));
    assert!(cache.content(pane, first, PROMPT_TEXT).is_none());
    assert_eq!(cache.content(pane, second, PROMPT_TEXT), Some(&content()));
    cache.forget_pane(pane);
    assert!(!cache.has_pane(pane));
    assert_eq!(
        cache.content(pane + 1, first, PROMPT_TEXT),
        Some(&content())
    );
    cache.remember(
        pane + 1,
        first,
        PROMPT_TEXT,
        &InputContent::new(PROMPT_TEXT),
    );
    assert!(cache.panes.is_empty());
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Remove,
    Clear,
    EscapeParked,
    NewSession,
    Reconnect,
    Error,
    Sent,
    Edit,
    Park,
    CancelEdit,
    ResumeOffline,
}

#[gpui::test]
fn queue_notifications_reconcile_removals_and_preserve_waiting_drafts(cx: &mut TestAppContext) {
    for (change, retained) in [
        (Change::Remove, false),
        (Change::Clear, false),
        (Change::EscapeParked, false),
        (Change::NewSession, false),
        (Change::Reconnect, false),
        (Change::Error, false),
        (Change::Sent, false),
        (Change::Edit, false),
        (Change::Park, true),
        (Change::CancelEdit, true),
        (Change::ResumeOffline, true),
    ] {
        let fixture = workspace_with_agent_chat(cx);
        let pane = fixture.pane();
        let (view, id) = cache_prompt(&fixture, pane, cx);
        // Keep the receiver alive so the Sent case proves a successful wire
        // handoff, rather than the failure path discarding the same queue.
        let (handle, mut probe) = AcpSessionHandle::detached_for_test();
        fixture.workspace.update(cx, |ws, cx| match change {
            Change::Sent => {
                view.update(cx, |view, _| {
                    view.attach_handle(handle);
                    view.set_status_for_test(AgentSessionStatus::Connected);
                });
                ws.fold_agent_chat_event(
                    pane,
                    AcpEvent::TurnEnded {
                        stop_reason: "end_turn".into(),
                        completed_normally: true,
                        usage: None,
                    },
                    cx,
                );
            }
            Change::Error => {
                ws.fold_agent_chat_event(
                    pane,
                    AcpEvent::Error(daruda_acp::AcpFailure::unclassified("session failed")),
                    cx,
                );
            }
            _ => view.update(cx, |view, cx| match change {
                Change::Remove => view.remove_queued(id, cx),
                Change::Clear => view.clear_queue(cx),
                Change::EscapeParked => {
                    view.cancel_turn(cx);
                    view.handle_escape(cx);
                }
                Change::NewSession => view.reset_for_new_session(cx),
                Change::Reconnect => view.retry_for_reconnect(None, cx),
                Change::Edit => {
                    view.begin_edit(id, cx);
                    view.send_prompt_text("replacement".into(), cx);
                }
                Change::Park => view.cancel_turn(cx),
                Change::CancelEdit => {
                    view.begin_edit(id, cx);
                    view.cancel_edit(cx);
                }
                Change::ResumeOffline => {
                    view.cancel_turn(cx);
                    view.resume_queue(cx);
                }
                Change::Sent | Change::Error => unreachable!(),
            }),
        });
        cx.run_until_parked();
        fixture.workspace.read_with(cx, |ws, _| {
            let lane = ws.lane_ref_for_pane(pane).unwrap();
            let cache = &ws.lane_scoped.get(&lane).unwrap().queued_input_cache;
            assert_eq!(cache.has_pane(pane), retained, "{change:?}");
            assert_eq!(
                cache.content(pane, id, PROMPT_TEXT),
                retained.then(content).as_ref(),
                "{change:?}"
            );
        });
        if matches!(change, Change::Sent) {
            assert_eq!(probe.drain_prompts(), vec![PROMPT_TEXT], "{change:?}");
        }
    }
}

#[gpui::test]
fn background_queue_cleanup_uses_its_own_lane_and_does_not_recreate_removed_state(
    cx: &mut TestAppContext,
) {
    let fixture = workspace_with_agent_chat(cx);
    let first_pane = fixture.pane();
    let (first_view, first_id) = cache_prompt(&fixture, first_pane, cx);
    let home = fixture
        .workspace
        .read_with(cx, |ws, _| ws.lane_ref_for_pane(first_pane).unwrap());
    let other_pane = cx
        .update_window(fixture.window.into(), |_, window, cx| {
            fixture.workspace.update(cx, |ws, cx| {
                ws.open_agent_chat_panes_in_fresh_lanes_for_test(1, window, cx)[0]
            })
        })
        .unwrap();
    let (_, other_id) = cache_prompt(&fixture, other_pane, cx);
    assert_eq!(first_id, other_id, "prompt ids are scoped to each view");
    fixture.workspace.read_with(cx, |ws, _| {
        assert_ne!(ws.active_ref(), home);
        assert_eq!(
            ws.queued_input_content(first_pane, first_id, PROMPT_TEXT),
            content()
        );
    });

    first_view.update(cx, |view, cx| view.clear_queue(cx));
    cx.run_until_parked();
    fixture.workspace.read_with(cx, |ws, _| {
        assert!(
            !ws.lane_scoped
                .get(&home)
                .unwrap()
                .queued_input_cache
                .has_pane(first_pane)
        );
        assert_eq!(
            ws.queued_input_content(other_pane, other_id, PROMPT_TEXT),
            content()
        );
    });
    fixture.workspace.update(cx, |ws, _| {
        ws.lane_scoped.remove(&home);
    });
    first_view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    fixture.workspace.read_with(cx, |ws, _| {
        assert!(!ws.lane_scoped.contains_key(&home));
    });
}

#[gpui::test]
fn orchestrator_references_follow_the_session_across_worktree_wrappers(cx: &mut TestAppContext) {
    let fixture = workspace_with_agent_chat(cx);
    let (pane, id, home) = cx
        .update_window(fixture.window.into(), |_, window, cx| {
            fixture.workspace.update(cx, |ws, cx| {
                let pane = ws
                    .seed_orchestrator_chat_pane_unrevealed_for_test(
                        ws.open_agent_id(None),
                        fixture.root().to_path_buf(),
                        daruda_store::accounts::AccountSelection::SystemDefault,
                        None,
                        window,
                        cx,
                    )
                    .expect("orchestrator seeded");
                assert!(ws.show_orchestrator_tab(window, cx));
                let before = ws.snapshot_composer_queue(cx);
                let view = ws.agent_chat_view(pane).unwrap().clone();
                let id = view.update(cx, |view, cx| {
                    view.send_prompt_text(PROMPT_TEXT.into(), cx);
                    view.queue().last_prompt_id().unwrap()
                });
                ws.cache_queued_input(before, &content(), PROMPT_TEXT, cx);
                (pane, id, ws.active_ref())
            })
        })
        .unwrap();
    cx.run_until_parked();
    cx.update_window(fixture.window.into(), |_, window, cx| {
        fixture.workspace.update(cx, |ws, cx| {
            ws.open_agent_chat_panes_in_fresh_lanes_for_test(1, window, cx);
            ws.lane_scoped.remove(&home);
            assert!(!ws.orchestrator_tab_is_visible());
            assert_eq!(ws.queued_input_content(pane, id, PROMPT_TEXT), content());
            assert!(ws.show_orchestrator_tab(window, cx));
            ws.begin_edit_queued_prompt(pane, id, window, cx);
            assert_eq!(ws.input_dock.input.read(cx).content(), content());
            ws.cancel_edit_queued_prompt(pane, window, cx);
        });
    })
    .unwrap();
    let view = fixture
        .workspace
        .read_with(cx, |ws, _| ws.agent_chat_view(pane).unwrap().clone());
    view.update(cx, |view, cx| view.clear_queue(cx));
    cx.run_until_parked();
    fixture.workspace.read_with(cx, |ws, _| {
        assert!(
            !ws.orchestrator
                .chat
                .as_ref()
                .unwrap()
                .queued_input_cache
                .has_pane(pane)
        );
    });
}
