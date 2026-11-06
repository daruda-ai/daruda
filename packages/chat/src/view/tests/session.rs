//! One session's lifecycle: the replay gate, resume, failures and a fresh
//! session, the mode it remembers, and the permission requests it raises.

use super::*;

/// A `session/load` replay coalesces row rebuilding until it reaches a
/// terminal resume signal. The same gate must also release on failure paths so
/// replayed content is not left invisible.
#[gpui::test]
fn replay_gate_releases_on_connected_error_or_abort(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{AcpEvent, ChatItem};

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            // Simulate a resume mid-replay: gate open on the id the load asked
            // for, items populated by the replayed updates, rows not yet
            // projected.
            view.replay = crate::view::Replay::Loading {
                requested: "sess-1".into(),
            };
            view.items = vec![ChatItem::UserText("q".into()), assistant_text_item("a")];
            view.rows.clear();

            // A non-terminal event during the replay must NOT rebuild rows.
            view.apply_event(AcpEvent::Notice("still loading".into()), "", false, cx);
            assert!(view.is_replaying(), "gate stays set until Connected/Error");
            assert!(
                view.rows.is_empty(),
                "row rebuild is coalesced while replaying"
            );
            assert_eq!(view.items.len(), 2, "items still accumulate during replay");

            // Connected clears the gate and runs the catch-up rebuild.
            view.apply_event(
                AcpEvent::Connected {
                    program: None,
                    session_id: "sess-1".into(),
                    modes: None,
                    config_options: Vec::new(),
                    capabilities: Default::default(),
                    login_methods: Vec::new(),
                },
                "",
                false,
                cx,
            );
            assert!(!view.is_replaying(), "Connected releases the gate");
            assert_eq!(view.session_id.as_deref(), Some("sess-1"));
            assert!(
                !view.rows.is_empty(),
                "the catch-up rebuild projects the replayed items"
            );
            assert_eq!(
                view.items.len(),
                2,
                "resume keeps the replayed conversation"
            );

            // The end-of-stream guard releases a gate stuck with no terminal
            // event, projecting whatever accumulated.
            view.replay = crate::view::Replay::Loading {
                requested: "sess-1".into(),
            };
            view.items = vec![ChatItem::UserText("q".into())];
            view.rows.clear();
            view.abort_restore(cx);
            assert!(!view.is_replaying(), "abort_restore releases the gate");
            assert!(
                !view.rows.is_empty(),
                "abort_restore projects the accumulated items"
            );
        })
        .unwrap();
}

/// A `session/load` the agent cannot honour is silently downgraded to a fresh
/// `session/new`, which answers with a *different* id. The gate carries the id
/// it asked for, so that downgrade is detectable: the pane must run
/// fresh-session setup instead of keeping the prior session's title.
#[gpui::test]
fn a_downgraded_resume_is_not_mistaken_for_a_real_one(cx: &mut gpui::TestAppContext) {
    use daruda_acp::AcpEvent;

    fn connected(session_id: &str) -> AcpEvent {
        AcpEvent::Connected {
            program: None,
            session_id: session_id.into(),
            modes: None,
            config_options: Vec::new(),
            capabilities: Default::default(),
            login_methods: Vec::new(),
        }
    }

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            // Arm the gate the way a connect does — through the seam, so this
            // also pins that `begin_connect` opens it on the id it was handed.
            view.begin_connect(Some("sess-old".into()), cx);
            assert!(view.is_replaying(), "a resume target opens the gate");
            view.session_title = Some("prior conversation".into());
            // The agent answered the load with an id we never asked for.
            view.apply_event(connected("sess-new"), "", false, cx);
            assert_eq!(view.session_id.as_deref(), Some("sess-new"));
            assert!(
                view.session_title.is_none(),
                "a downgraded resume runs fresh-session setup"
            );

            // The same reply against the id the load asked for is a real resume.
            view.begin_connect(Some("sess-old".into()), cx);
            view.session_title = Some("prior conversation".into());
            view.apply_event(connected("sess-old"), "", false, cx);
            assert_eq!(
                view.session_title.as_deref(),
                Some("prior conversation"),
                "an honoured resume keeps the replayed session title"
            );

            // A fresh connect names no resume target, so it opens no gate.
            view.begin_connect(None, cx);
            assert!(
                !view.is_replaying(),
                "a fresh session/new has nothing to replay"
            );
        })
        .unwrap();
}

#[gpui::test]
fn transport_eof_localizes_and_preserves_the_session_for_reconnect(cx: &mut gpui::TestAppContext) {
    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.session_id = Some("saved-session".into());
            view.status = crate::view::AgentSessionStatus::Connected;
            assert!(view.needs_disconnect_error());
            view.set_turn_in_flight();
            view.apply_event(
                daruda_acp::AcpEvent::Error(daruda_acp::AcpFailure::TransportClosed {
                    message: "Incoming transport closed".into(),
                }),
                "",
                false,
                cx,
            );
            let crate::view::AgentSessionStatus::Error { message, remedy } = &view.status else {
                panic!("EOF must terminate the session");
            };
            assert_eq!(*remedy, daruda_acp::Remedy::Retry);
            assert_eq!(message, &crate::strings::agent_chat::transport_closed());
            assert_ne!(message, "Incoming transport closed");
            assert_eq!(view.session_id.as_deref(), Some("saved-session"));
            assert!(view.turn_is_idle());
            assert!(view.any_handle().is_none());
            assert!(
                !view.needs_disconnect_error(),
                "do not emit a duplicate terminal failure"
            );
        })
        .unwrap();
}

#[gpui::test]
fn preparation_failure_releases_replay_without_losing_the_resume_target(
    cx: &mut gpui::TestAppContext,
) {
    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.session_id = Some("saved-session".into());
            view.begin_connect(Some("saved-session".into()), cx);
            assert!(view.is_replaying());
            view.apply_event(
                daruda_acp::AcpEvent::Error(daruda_acp::AcpFailure::AdapterInstall {
                    kind: daruda_acp::preparation::PreparationKind::Network,
                    message: "npm ENOTFOUND".into(),
                }),
                "",
                false,
                cx,
            );
            assert!(!view.is_replaying());
            assert!(view.turn_is_idle());
            assert!(view.any_handle().is_none());
            assert_eq!(view.session_id.as_deref(), Some("saved-session"));
            let crate::view::AgentSessionStatus::Error { message, remedy } = &view.status else {
                panic!("preparation failure must terminate the attempt");
            };
            assert_eq!(*remedy, daruda_acp::Remedy::Retry);
            assert_eq!(
                message,
                &crate::strings::agent_chat::adapter_install_failed()
            );
        })
        .unwrap();
}

#[gpui::test]
fn invalid_package_has_localized_guidance_without_a_misleading_retry(
    cx: &mut gpui::TestAppContext,
) {
    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.apply_event(
                daruda_acp::AcpEvent::Error(daruda_acp::AcpFailure::AdapterInstall {
                    kind: daruda_acp::preparation::PreparationKind::InvalidPackage,
                    message: "unsupported bin entry".into(),
                }),
                "",
                false,
                cx,
            );
            let crate::view::AgentSessionStatus::Error { message, remedy } = &view.status else {
                panic!("invalid package must surface a failure");
            };
            assert_ne!(*remedy, daruda_acp::Remedy::Retry);
            assert_eq!(
                message,
                &crate::strings::agent_chat::adapter_setup_required()
            );
        })
        .unwrap();
}

/// `/clear` resets the local session model before the workspace starts the
/// fresh connection.
#[gpui::test]
fn reset_for_new_session_clears_conversation_state(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{ChatItem, PlanEntryView, PlanPriority, PlanStatus};

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.session_id = Some("abc".to_string());
            view.items.push(ChatItem::UserText("hi".into()));
            view.items.push(ChatItem::UserText("again".into()));
            view.set_turn_in_flight();
            view.plan.push(PlanEntryView {
                content: "step 1".into(),
                priority: PlanPriority::Medium,
                status: PlanStatus::Pending,
            });
            view.fold
                .toggle(FoldKey::Thinking(1), FoldContext::past(true));
            assert!(
                !view
                    .fold
                    .is_expanded(&FoldKey::Thinking(1), FoldContext::past(true)),
                "sanity: override collapsed the block while active"
            );
            view.fold.set_mode(FoldPreset::Expanded.mode());
            view.activity.turn_records.insert(
                1,
                crate::view::TurnRecord {
                    worked_for: std::time::Duration::from_secs(9),
                    finished_at: chrono::Local::now(),
                    output_tokens: Some(42),
                },
            );

            view.reset_for_new_session(cx);

            assert!(view.items.is_empty(), "reset clears the conversation items");
            assert!(
                view.activity.turn_records.is_empty(),
                "turn records are keyed by item index with no session identity, \
                 so one left behind would label the next conversation's rows"
            );

            assert!(
                view.rows.is_empty(),
                "reset splices the projected rows to 0"
            );
            assert_eq!(
                view.session_id, None,
                "reset clears the persisted session id"
            );
            assert_eq!(
                view.status,
                crate::view::AgentSessionStatus::Connecting,
                "reset parks the view in Connecting for the fresh session/new"
            );
            assert!(view.turn_is_idle(), "reset clears the in-flight turn flag");
            assert!(view.plan.is_empty(), "reset clears the execution plan");
            assert!(
                view.fold
                    .is_expanded(&FoldKey::Thinking(1), FoldContext::past(true)),
                "reset drops fold overrides back to the mode's rule"
            );
            assert_eq!(
                view.fold.chosen_mode(),
                Some(FoldPreset::Expanded.mode()),
                "reset keeps the pane's fold mode: it is a user preference, not                  conversation state, and wiping it here also erases it from disk"
            );
        })
        .unwrap();
}

// Which mode a pane remembers as its own pick.

/// Only the user's pick is remembered: a mode the adapter reports — at connect
/// or by switching on its own — must not outrank the agent's `default_mode` on
/// the next connect.
#[gpui::test]
fn only_a_user_mode_pick_is_remembered(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{AcpEvent, ModeStateView, SessionCapabilitiesView, SessionModeView};

    let modes = |current: &str| ModeStateView {
        available: ["default", "plan", "bypassPermissions"]
            .into_iter()
            .map(|id| SessionModeView {
                id: id.into(),
                name: id.into(),
                description: None,
            })
            .collect(),
        current: current.into(),
    };
    let window = make_test_view(cx);
    window
        .update(cx, |v, _, cx| {
            v.apply_event(
                AcpEvent::Connected {
                    program: None,
                    session_id: "s1".into(),
                    modes: Some(modes("default")),
                    config_options: Vec::new(),
                    capabilities: SessionCapabilitiesView::default(),
                    login_methods: Vec::new(),
                },
                "",
                false,
                cx,
            );
            v.apply_event(
                AcpEvent::ModeChanged {
                    state: modes("plan"),
                },
                "",
                false,
                cx,
            );
            assert_eq!(
                v.picked_mode_id, None,
                "a mode the adapter reported is not a pick"
            );

            v.set_mode("bypassPermissions".into(), cx);
            v.apply_event(
                AcpEvent::ModeChanged {
                    state: modes("default"),
                },
                "",
                false,
                cx,
            );
            assert_eq!(
                v.picked_mode_id.as_deref(),
                Some("bypassPermissions"),
                "the adapter switching on its own leaves the user's pick standing"
            );
        })
        .unwrap();
}

#[gpui::test]
fn mode_state_updates_replace_and_survive_config_refresh(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{
        AcpEvent, ConfigChoiceView, ConfigOptionCategoryView, ConfigOptionView, ModeStateView,
        SessionModeView,
    };

    let mode = |id: &str| SessionModeView {
        id: id.to_string(),
        name: id.to_uppercase(),
        description: None,
    };

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.session_config.modes = Some(ModeStateView {
                available: vec![
                    SessionModeView {
                        id: "auto".to_string(),
                        name: "Auto".to_string(),
                        description: None,
                    },
                    SessionModeView {
                        id: "plan".to_string(),
                        name: "Plan".to_string(),
                        description: Some("Plan mode".to_string()),
                    },
                ],
                current: "auto".to_string(),
            });
            view.set_mode("plan".to_string(), cx);
            assert_eq!(
                view.session_config
                    .modes
                    .as_ref()
                    .expect("modes were injected")
                    .current,
                "plan",
                "set_mode flips current immediately (optimistic)"
            );

            // Connect-time state: the model in use advertises no `auto`.
            view.session_config.modes = Some(ModeStateView {
                available: vec![mode("default"), mode("plan")],
                current: "plan".to_string(),
            });
            // The agent switched to a model that supports `auto` and
            // re-advertised — list and current mode both move.
            view.apply_event(
                AcpEvent::ModeChanged {
                    state: ModeStateView {
                        available: vec![mode("auto"), mode("default"), mode("plan")],
                        current: "auto".to_string(),
                    },
                },
                "",
                false,
                cx,
            );
            let modes = view
                .session_config
                .modes
                .as_ref()
                .expect("modes stay advertised");
            assert_eq!(
                modes
                    .available
                    .iter()
                    .map(|m| m.id.as_str())
                    .collect::<Vec<_>>(),
                ["auto", "default", "plan"],
                "the rebuilt list replaces the connect-time one"
            );
            assert_eq!(
                modes.current, "auto",
                "a mode absent from the connect-time list still applies"
            );

            view.apply_event(
                AcpEvent::ConfigOptionsChanged(vec![ConfigOptionView {
                    id: "model".to_string(),
                    name: "Model".to_string(),
                    description: None,
                    category: ConfigOptionCategoryView::Model,
                    kind: daruda_acp::ConfigOptionKindView::Select {
                        current_value: "sonnet".to_string(),
                        options: vec![ConfigChoiceView {
                            value: "sonnet".to_string(),
                            name: "Sonnet".to_string(),
                            description: None,
                        }],
                    },
                }]),
                "",
                false,
                cx,
            );
            assert_eq!(
                view.session_config
                    .modes
                    .as_ref()
                    .expect("modes untouched")
                    .current,
                "auto"
            );
            assert_eq!(view.session_config.config_options.len(), 1);
        })
        .unwrap();
}

// Permission requests: routing a resolution by id and draining on Stop.

#[gpui::test]
fn permission_settlements_notify_the_host_once_per_request(cx: &mut gpui::TestAppContext) {
    use crate::view::{AgentChatEvent, PermissionOutcome};
    use std::{cell::RefCell, rc::Rc};
    let window = make_test_view(cx);
    let view = window.root(cx).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &AgentChatEvent, _| {
            if let AgentChatEvent::PermissionSettled {
                session,
                request_id,
                outcome,
            } = event
            {
                observed
                    .borrow_mut()
                    .push((*session, *request_id, *outcome));
            }
        })
        .detach();
    });
    view.update(cx, |view, cx| {
        view.items = vec![permission_card(1), permission_card(2), permission_card(3)];
        view.pending_permissions.extend([1, 2, 3]);
        view.respond_permission(
            1,
            "allow_once".into(),
            daruda_acp::PermissionKindView::AllowOnce,
            cx,
        );
        view.respond_permission(
            1,
            "reject_once".into(),
            daruda_acp::PermissionKindView::RejectOnce,
            cx,
        );
    });
    let session = view.read_with(cx, |view, _| view.permission_session());
    assert_eq!(*events.borrow(), [(session, 1, PermissionOutcome::Allowed)]);
    view.update(cx, |view, cx| view.cancel_turn(cx));
    assert_eq!(
        events.borrow().len(),
        3,
        "Stop owes a settlement for every remaining request"
    );
    view.update(cx, |view, cx| view.cancel_turn(cx));
    assert_eq!(events.borrow().len(), 3, "settling again emits nothing");
    let mut cancelled: Vec<_> = events
        .borrow()
        .iter()
        .filter(|(_, _, outcome)| *outcome == PermissionOutcome::Cancelled)
        .copied()
        .collect();
    cancelled.sort_by_key(|(_, request, _)| *request);
    assert_eq!(
        cancelled,
        [
            (session, 2, PermissionOutcome::Cancelled),
            (session, 3, PermissionOutcome::Cancelled)
        ]
    );
}

#[gpui::test]
fn replacing_a_session_cancels_the_old_scope_before_request_ids_can_repeat(
    cx: &mut gpui::TestAppContext,
) {
    use crate::view::{AgentChatEvent, PermissionOutcome};
    use std::{cell::RefCell, rc::Rc};
    let window = make_test_view(cx);
    let view = window.root(cx).unwrap();
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    cx.update(|cx| {
        cx.subscribe(&view, move |_, event: &AgentChatEvent, _| {
            if let AgentChatEvent::PermissionSettled {
                session,
                request_id,
                outcome,
            } = event
            {
                observed
                    .borrow_mut()
                    .push((*session, *request_id, *outcome));
            }
        })
        .detach();
    });
    let old = view.update(cx, |view, cx| {
        view.items = vec![permission_card(0)];
        view.pending_permissions.insert(0);
        let old = view.permission_session();
        view.reset_for_new_session(cx);
        old
    });
    let new = view.update(cx, |view, cx| {
        view.items = vec![permission_card(0)];
        view.pending_permissions.insert(0);
        view.respond_permission(
            0,
            "reject_once".into(),
            daruda_acp::PermissionKindView::RejectOnce,
            cx,
        );
        view.permission_session()
    });
    assert_ne!(old, new);
    assert_eq!(
        *events.borrow(),
        [
            (old, 0, PermissionOutcome::Cancelled),
            (new, 0, PermissionOutcome::Rejected)
        ]
    );
}

#[gpui::test]
fn permission_resolution_routes_by_id_reprojects_and_cancel_drains(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{ChatItem, PermissionKindView, PermissionResolution, ToolStatusView};

    let window = make_test_view(cx);
    window
        .update(cx, |view, live_window, cx| {
            view.items = vec![permission_card(100), permission_card(200)];
            view.pending_permissions.insert(100);
            view.pending_permissions.insert(200);

            view.respond_permission(
                100,
                "allow_once".to_string(),
                PermissionKindView::AllowOnce,
                cx,
            );

            let ChatItem::Permission(first) = &view.items[0] else {
                panic!("expected first permission card");
            };
            let ChatItem::Permission(second) = &view.items[1] else {
                panic!("expected second permission card");
            };
            assert_eq!(
                first.resolved,
                Some(PermissionResolution::Chosen("allow_once".to_string())),
                "the answered card resolves by id"
            );
            assert_eq!(
                second.resolved, None,
                "answering one permission leaves the other live"
            );
            assert!(!view.is_permission_outstanding(100));
            assert!(view.is_permission_outstanding(200));

            view.respond_permission(
                200,
                "reject_once".to_string(),
                PermissionKindView::RejectOnce,
                cx,
            );
            let ChatItem::Permission(second) = &view.items[1] else {
                panic!("expected second permission card");
            };
            assert_eq!(
                second.resolved,
                Some(PermissionResolution::Chosen("reject_once".to_string()))
            );
            assert!(!view.has_pending_permission());

            view.items = vec![
                ChatItem::UserText("q".into()),
                tool_call("a", ToolStatusView::Completed, None),
                tool_call("b", ToolStatusView::Completed, None),
                permission_card(7),
            ];
            view.pending_permissions.insert(7);
            view.set_all_folds(false, live_window, cx);
            assert!(
                view.rows
                    .iter()
                    .any(|r| matches!(r.kind, RowKind::AgentItem(3)) && !r.hidden),
                "a pending permission stays visible under a collapsed response"
            );

            view.respond_permission(
                7,
                "allow_once".to_string(),
                PermissionKindView::AllowOnce,
                cx,
            );
            assert!(
                view.rows
                    .iter()
                    .any(|r| matches!(r.kind, RowKind::AgentItem(3)) && r.hidden),
                "a resolved permission folds back into the collapsed response immediately"
            );

            view.items = vec![
                permission_card(10),
                permission_card(20),
                permission_card(30),
            ];
            view.pending_permissions.extend([10, 20, 30]);
            view.set_turn_in_flight();
            view.cancel_turn(cx);

            // Stop also appends its marker, so match the cards by variant
            // rather than asserting every item is one.
            let cards: Vec<_> = view
                .items
                .iter()
                .filter_map(|item| match item {
                    ChatItem::Permission(card) => Some(card),
                    _ => None,
                })
                .collect();
            assert_eq!(cards.len(), 3, "expected the three permission cards");
            for card in cards {
                assert_eq!(
                    card.resolved,
                    Some(PermissionResolution::Cancelled),
                    "Stop cancels every outstanding permission"
                );
            }
            assert!(!view.has_pending_permission());
        })
        .unwrap();
}

fn party(agent: &str, account: &str) -> daruda_acp::handoff::HandoffParty {
    daruda_acp::handoff::HandoffParty {
        agent: agent.into(),
        account: Some(account.into()),
    }
}

/// Switch `view` from the work account to the personal one.
fn switch_account(
    view: &mut crate::view::AgentChatView,
    cx: &mut gpui::Context<crate::view::AgentChatView>,
) {
    view.begin_handoff(
        party("Claude Code", "work"),
        party("Claude Code", "personal"),
        cx,
    );
}

/// The divider a switch from work to personal leaves on screen.
fn live_divider() -> daruda_acp::ChatItem {
    use daruda_acp::handoff::{HandoffItem, HandoffMark};
    daruda_acp::ChatItem::Handoff(HandoffItem::live(HandoffMark {
        from: party("Claude Code", "work"),
        to: party("Claude Code", "personal"),
    }))
}

/// Whether `wire` is the recap block, then `prompt`.
fn briefed(wire: &str, prompt: &str) -> bool {
    wire.starts_with("<conversation_handoff") && wire.ends_with(&format!("\n\n---\n\n{prompt}"))
}

/// A switch keeps the conversation on screen, marks where it happened, and
/// sends the recap once in front of the next prompt only.
#[gpui::test]
fn begin_handoff_keeps_the_conversation_and_briefs_the_next_prompt(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{ChatItem, PlanEntryView, PlanPriority, PlanStatus};

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.session_id = Some("old".to_string());
            view.items.push(ChatItem::UserText("hi".into()));
            view.activity.turn_records.insert(
                0,
                crate::view::TurnRecord {
                    worked_for: std::time::Duration::from_secs(3),
                    finished_at: chrono::Local::now(),
                    output_tokens: None,
                },
            );
            view.plan.push(PlanEntryView {
                content: "step".into(),
                priority: PlanPriority::Medium,
                status: PlanStatus::Pending,
            });

            switch_account(view, cx);

            assert_eq!(
                view.items,
                vec![ChatItem::UserText("hi".into()), live_divider()]
            );
            assert!(
                view.activity.turn_records.contains_key(&0),
                "records index items that are still on screen"
            );
            assert_eq!(view.session_id, None, "the old session is not resumed");
            assert!(view.plan.is_empty(), "the plan was the old session's");
            assert_eq!(
                view.status,
                crate::view::AgentSessionStatus::Connecting,
                "parked for the fresh session/new"
            );
            let first = view.wire_text_for_test("go on");
            assert!(briefed(&first, "go on"), "{first}");
            assert!(
                first.contains("User:\nhi"),
                "the recap records the conversation"
            );
            assert_eq!(view.wire_text_for_test("again"), "again", "only once");
        })
        .unwrap();
}

/// Retrying a connect with nothing to resume (a switch whose new session
/// failed) must not wipe the conversation: no replay would bring it back. The
/// failure being retried goes; the recap is still owed to the next prompt.
#[gpui::test]
fn retry_without_a_resume_target_keeps_the_conversation(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{AcpFailure, ChatItem};

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("hi".into()));
            switch_account(view, cx);
            view.items.push(ChatItem::Failure(AcpFailure::unclassified(
                "adapter exited",
            )));

            view.retry_for_reconnect(None, cx);

            assert_eq!(
                view.items,
                vec![ChatItem::UserText("hi".into()), live_divider()]
            );
            assert!(briefed(&view.wire_text_for_test("go"), "go"));
        })
        .unwrap();
}

/// Once the switched-to session has an id, a reconnect resumes it — and its
/// replay only covers what came after the divider. What came before must
/// survive, or a dropped connection would erase the conversation the switch
/// carried over.
#[gpui::test]
fn resuming_the_switched_to_session_keeps_the_conversation_before_the_switch(
    cx: &mut gpui::TestAppContext,
) {
    use daruda_acp::ChatItem;

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("before".into()));
            switch_account(view, cx);
            view.session_id = Some("new".into());
            view.items.push(ChatItem::UserText("after".into()));

            view.retry_for_reconnect(Some("new".into()), cx);

            assert_eq!(
                view.items,
                vec![ChatItem::UserText("before".into()), live_divider()],
                "the resumed session replays only its own history"
            );
        })
        .unwrap();
}

/// `/clear` after a switch starts over: the recap describes the conversation
/// it cleared and must not reach the next session. A pane's standing briefing
/// is not about that conversation and stays.
#[gpui::test]
fn clear_drops_a_pending_recap_but_not_the_briefing(cx: &mut gpui::TestAppContext) {
    use daruda_acp::ChatItem;

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("hi".into()));
            view.set_briefing("BRIEF".into());
            switch_account(view, cx);
            let peek = view.peek_wire_text_for_test("peek");
            assert!(
                briefed(&peek, "BRIEF\n\n---\n\npeek"),
                "the recap leads so a replay finds its block first: {peek}"
            );

            view.reset_for_new_session(cx);

            assert_eq!(view.wire_text_for_test("go"), "BRIEF\n\n---\n\ngo");
        })
        .unwrap();
}

/// A first send that never reaches the agent has not told it anything: the
/// recap stays for the prompt that does.
#[gpui::test]
fn a_failed_first_send_keeps_the_recap(cx: &mut gpui::TestAppContext) {
    use daruda_acp::ChatItem;

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("hi".into()));
            switch_account(view, cx);
            let (handle, probe) = daruda_acp::AcpSessionHandle::detached_for_test();
            view.status = crate::view::AgentSessionStatus::Connected;
            view.attach_handle(handle);
            // A closed connection: the send cannot be handed over.
            drop(probe);

            view.send_prompt_text("go on".into(), cx);

            assert!(
                matches!(view.status, crate::view::AgentSessionStatus::Error { .. }),
                "the failed handoff parks the pane for a retry"
            );
            assert!(briefed(&view.peek_wire_text_for_test("go on"), "go on"));
        })
        .unwrap();
}

/// Switching again before anyone spoke replaces the trailing divider instead
/// of stacking a second one; and the failure that prompted a switch stays in
/// the record above it.
#[gpui::test]
fn a_second_switch_replaces_the_trailing_divider_and_keeps_the_failure(
    cx: &mut gpui::TestAppContext,
) {
    use daruda_acp::handoff::{HandoffItem, HandoffMark};
    use daruda_acp::{AcpFailure, ChatItem};

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("hi".into()));
            view.items
                .push(ChatItem::Failure(AcpFailure::unclassified("session limit")));
            switch_account(view, cx);
            view.begin_handoff(
                party("Claude Code", "personal"),
                party("Claude Code", "team"),
                cx,
            );

            assert_eq!(view.items.len(), 3, "{:?}", view.items);
            assert!(matches!(view.items[1], ChatItem::Failure(_)));
            assert_eq!(
                view.items[2],
                ChatItem::Handoff(HandoffItem::live(HandoffMark {
                    from: party("Claude Code", "work"),
                    to: party("Claude Code", "team"),
                }))
            );
            let wire = view.wire_text_for_test("go");
            assert!(briefed(&wire, "go") && !wire.contains("moved to"), "{wire}");
        })
        .unwrap();
}

/// A restart before the first prompt after a switch: the pane comes back with
/// the divider — its record standing in for the conversation the empty new
/// session cannot replay — and the recap still owed.
#[gpui::test]
fn a_restored_pending_handoff_brings_back_the_divider_and_the_recap(cx: &mut gpui::TestAppContext) {
    use daruda_acp::ChatItem;

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("hi".into()));
            switch_account(view, cx);
            let saved = view.pending_handoff().expect("owed").to_owned();

            view.reset_for_new_session(cx);
            view.restore_pending_handoff(saved.clone());

            let [ChatItem::Handoff(divider)] = view.items.as_slice() else {
                panic!("{:?}", view.items);
            };
            assert_eq!(divider.record.as_deref(), Some("User:\nhi"));
            assert_eq!(view.pending_handoff(), Some(saved.as_str()));
            assert!(briefed(&view.wire_text_for_test("go"), "go"));
        })
        .unwrap();
}

/// The saved recap can be older than the session: if the session replays a
/// prompt after the divider, that prompt carried the recap, and it must not
/// go out twice.
#[gpui::test]
fn a_replayed_prompt_settles_a_restored_recap(cx: &mut gpui::TestAppContext) {
    use daruda_acp::{AcpEvent, ChatItem};

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items.push(ChatItem::UserText("hi".into()));
            switch_account(view, cx);
            let saved = view.pending_handoff().unwrap().to_owned();
            view.reset_for_new_session(cx);
            view.restore_pending_handoff(saved);
            view.replay = crate::view::Replay::Loading {
                requested: "s".into(),
            };
            view.items.push(ChatItem::UserText("go".into()));

            view.apply_event(
                AcpEvent::Connected {
                    program: None,
                    session_id: "s".into(),
                    modes: None,
                    config_options: Vec::new(),
                    capabilities: Default::default(),
                    login_methods: Vec::new(),
                },
                "",
                false,
                cx,
            );

            assert_eq!(view.pending_handoff(), None);
            assert_eq!(view.wire_text_for_test("next"), "next");
        })
        .unwrap();
}

/// A reconnect keeps the conversation above the last divider, and with it the
/// folds the person chose there; only choices about the replayed tail go.
#[gpui::test]
fn resuming_keeps_the_folds_above_the_divider(cx: &mut gpui::TestAppContext) {
    use daruda_acp::ChatItem;

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            let past = FoldContext::past(false);
            view.items.push(ChatItem::UserText("before".into()));
            view.items.push(ChatItem::Thinking {
                text: "kept".into(),
                streaming: false,
                message_id: None,
            });
            switch_account(view, cx);
            view.session_id = Some("new".into());
            view.items.push(ChatItem::Thinking {
                text: "replayed".into(),
                streaming: false,
                message_id: None,
            });
            let kept = FoldKey::Thinking(1);
            let tail = FoldKey::Thinking(3);
            let kept_default = view.fold.is_expanded(&kept, past);
            view.fold.toggle(kept.clone(), past);
            view.fold.toggle(tail.clone(), past);

            view.retry_for_reconnect(Some("new".into()), cx);

            assert_ne!(
                view.fold.is_expanded(&kept, past),
                kept_default,
                "the choice above the divider survives"
            );
            assert_eq!(
                view.fold.is_expanded(&tail, past),
                kept_default,
                "the replayed tail starts from the rule again"
            );
        })
        .unwrap();
}

/// Switching again on a pane a restart brought back before anyone spoke: the
/// divider is replaced, but its record — all that is left of the conversation
/// before it — stays on the card and goes out with the new recap.
#[gpui::test]
fn a_second_switch_on_a_restored_pane_keeps_its_record(cx: &mut gpui::TestAppContext) {
    use daruda_acp::ChatItem;

    let window = make_test_view(cx);
    window
        .update(cx, |view, _window, cx| {
            view.items
                .push(ChatItem::UserText("the earlier question".into()));
            switch_account(view, cx);
            let saved = view.pending_handoff().unwrap().to_owned();
            view.reset_for_new_session(cx);
            view.restore_pending_handoff(saved);

            view.begin_handoff(
                party("Claude Code", "personal"),
                party("Claude Code", "team"),
                cx,
            );

            let [ChatItem::Handoff(divider)] = view.items.as_slice() else {
                panic!("{:?}", view.items);
            };
            assert_eq!(
                divider.record.as_deref(),
                Some("User:\nthe earlier question")
            );
            let wire = view.wire_text_for_test("go");
            assert!(
                briefed(&wire, "go") && wire.contains("the earlier question"),
                "{wire}"
            );
        })
        .unwrap();
}
