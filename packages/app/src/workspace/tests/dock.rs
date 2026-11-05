use super::*;
use daruda_store::project::{LeftDockView, RightDockView};
use gpui::Focusable as _;

// ---- Dock integration ----

#[gpui::test]
fn dock_defaults_toggles_and_view_selection(cx: &mut TestAppContext) {
    let (_wh, ws) = build_workspace(cx);
    ws.update(cx, |ws, cx| {
        assert!(ws.docks.left.read(cx).is_open);
        assert!(!ws.docks.bottom.read(cx).is_open);
        assert!(!ws.docks.right.read(cx).is_open);

        // Panels are only ever counted — the tab strip owns the labels.
        assert_eq!(ws.docks.left.read(cx).panels.len(), 3);
        assert_eq!(ws.docks.bottom.read(cx).panels.len(), 1);
        assert_eq!(ws.docks.right.read(cx).panels.len(), 1);

        ws.docks.left.update(cx, |d, _| d.toggle());
        assert!(!ws.docks.left.read(cx).is_open);
        ws.docks.left.update(cx, |d, _| d.toggle());
        assert!(ws.docks.left.read(cx).is_open);

        ws.docks.bottom.update(cx, |d, _| d.toggle());
        assert!(ws.docks.bottom.read(cx).is_open);

        assert!(!ws.docks.right.read(cx).is_open);
        assert_eq!(ws.docks.right_view, RightDockView::Usage);
        ws.reveal_right_dock_view(RightDockView::Usage, cx);
        assert!(ws.docks.right.read(cx).is_open);
        assert_eq!(ws.docks.right_view, RightDockView::Usage);

        ws.reveal_right_dock_view(RightDockView::Skills, cx);
        assert!(ws.docks.right.read(cx).is_open);
        assert_eq!(ws.docks.right_view, RightDockView::Skills);

        ws.reveal_right_dock_view(RightDockView::Skills, cx);
        assert!(ws.docks.right.read(cx).is_open);
        ws.docks.right.update(cx, |d, _| d.toggle());
        assert!(!ws.docks.right.read(cx).is_open);
        ws.docks.right.update(cx, |d, _| d.toggle());
        assert!(ws.docks.right.read(cx).is_open);

        for view in [
            LeftDockView::GitChanges,
            LeftDockView::Files,
            LeftDockView::Files,
            LeftDockView::Lanes,
        ] {
            ws.set_left_dock_view(view, cx);
            assert_eq!(ws.docks.left_view, view);
        }
    });
}

/// The Input panel's stacked chrome is taller than the single-row macro
/// preset; showing it must lift the dock to fit, and the macro panel must be
/// free to shrink back afterwards.
#[gpui::test]
fn input_panel_lifts_the_bottom_dock_floor_to_its_one_row_height(cx: &mut TestAppContext) {
    let (_wh, ws) = build_workspace(cx);
    ws.update(cx, |ws, cx| {
        let one_row = layout::ops::bottom_dock_height_for_rows(
            1,
            daruda_config::font::InputFontConfig::default().row_height(),
        );
        let macro_floor = crate::ui::theme::DOCK_BOTTOM_MIN_H;
        assert!(macro_floor < one_row);

        let tab_id = ws.panels.tabs[0].id.clone();
        ws.set_active_panel_tab(tab_id, cx);
        ws.docks.bottom.update(cx, |d, _| d.resize(0.0));
        assert_eq!(ws.docks.bottom.read(cx).size, macro_floor);

        ws.activate_bottom_input(cx);
        assert_eq!(ws.docks.bottom.read(cx).size, one_row);
        ws.docks.bottom.update(cx, |d, _| d.resize(0.0));
        assert_eq!(ws.docks.bottom.read(cx).size, one_row);
    });
}

/// A multi-line draft floors the displayed height so a dock dragged to its
/// minimum still shows every row and the action row; the saved size stays the
/// user's, so a shorter draft lets the dock return to it.
#[gpui::test]
fn input_draft_rows_floor_the_displayed_bottom_dock(cx: &mut TestAppContext) {
    let (_wh, ws) = build_workspace(cx);
    ws.update(cx, |ws, cx| {
        let rows = |rows| {
            layout::ops::bottom_dock_height_for_rows(
                rows,
                daruda_config::font::InputFontConfig::default().row_height(),
            )
        };
        let tab_id = ws.panels.tabs[0].id.clone();
        ws.set_active_panel_tab(tab_id, cx);
        ws.activate_bottom_input(cx);
        ws.docks.bottom.update(cx, |d, _| d.resize(0.0));
        assert_eq!(ws.bottom_dock_display_size(cx), rows(1));

        ws.input_dock.line_count = 3;
        assert_eq!(ws.bottom_dock_display_size(cx), rows(3));
        assert_eq!(ws.docks.bottom.read(cx).size, rows(1));

        ws.input_dock.line_count = 1;
        assert_eq!(ws.bottom_dock_display_size(cx), rows(1));
    });
}

/// Expanding lifts the row cap past the configured rows, and the dock floor
/// with it; an emptied draft — what a send leaves — collapses it again.
#[gpui::test]
fn composer_expand_lifts_the_row_cap_and_an_emptied_draft_collapses_it(cx: &mut TestAppContext) {
    let (window_handle, ws) = build_workspace(cx);
    let rows = |rows| {
        layout::ops::bottom_dock_height_for_rows(
            rows,
            daruda_config::font::InputFontConfig::default().row_height(),
        )
    };
    let configured = ws.read_with(cx, |ws, _| usize::from(ws.mirrors.agent.input_max_rows));
    let draft = vec!["line"; configured + 3].join("\n");
    cx.update_window(window_handle.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            let tab_id = ws.panels.tabs[0].id.clone();
            ws.set_active_panel_tab(tab_id, cx);
            ws.activate_bottom_input(cx);
            ws.docks.bottom.update(cx, |d, _| d.resize(0.0));
            ws.input_dock
                .input
                .update(cx, |s, cx| s.set_value(draft, window, cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    ws.read_with(cx, |ws, cx| {
        assert_eq!(ws.input_dock.line_count, configured);
        assert_eq!(ws.bottom_dock_display_size(cx), rows(configured));
    });

    cx.update_window(window_handle.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| ws.set_composer_expanded(true, window, cx));
    })
    .unwrap();
    ws.read_with(cx, |ws, cx| {
        assert_eq!(ws.input_dock.line_count, configured + 3);
        assert_eq!(ws.bottom_dock_display_size(cx), rows(configured + 3));
        assert_eq!(ws.composer_expand(cx), layout::ComposerExpand::Collapse);
    });

    cx.update_window(window_handle.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.input_dock
                .input
                .update(cx, |s, cx| s.set_value("", window, cx));
        });
    })
    .unwrap();
    cx.run_until_parked();
    ws.read_with(cx, |ws, cx| {
        assert!(!ws.input_dock.expanded);
        assert_eq!(ws.input_dock.line_count, 1);
        assert_eq!(ws.bottom_dock_display_size(cx), rows(1));
    });
}

/// The toggle is clicked from inside the bottom dock's own view, and expanding
/// re-lays out that dock: a click that leased the dock while doing so aborted.
#[gpui::test]
fn clicking_the_composer_expand_toggle_expands_the_composer(cx: &mut TestAppContext) {
    let (window_handle, ws) = build_workspace(cx);
    let configured = ws.read_with(cx, |ws, _| usize::from(ws.mirrors.agent.input_max_rows));
    let draft = vec!["line"; configured + 3].join("\n");
    cx.update_window(window_handle.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.docks.bottom.update(cx, |d, _| d.open());
            let tab_id = ws.panels.tabs[0].id.clone();
            ws.set_active_panel_tab(tab_id, cx);
            ws.activate_bottom_input(cx);
            ws.input_dock
                .input
                .update(cx, |s, cx| s.set_value(draft, window, cx));
        });
    })
    .unwrap();

    let mut vcx = gpui::VisualTestContext::from_window(window_handle.into(), cx);
    // Two frames: the first paints the overflowing editor, the second offers
    // the toggle that paint revealed. Refresh bypasses the dock's view cache so
    // the button records its debug bounds.
    for _ in 0..2 {
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
    }
    vcx.run_until_parked();
    let toggle = vcx
        .debug_bounds("composer-expand")
        .expect("an overflowing draft offers the expand toggle");
    vcx.simulate_click(toggle.center(), gpui::Modifiers::default());
    vcx.run_until_parked();

    ws.read_with(&vcx, |ws, _| {
        assert!(ws.input_dock.expanded);
        assert_eq!(ws.input_dock.line_count, configured + 3);
    });
}

#[gpui::test]
fn dock_drag_resizes_clamps_tracks_positions_and_clears_stale(cx: &mut TestAppContext) {
    let (_wh, ws) = build_workspace(cx);
    ws.update(cx, |ws, cx| {
        ws.docks.left.update(cx, |d, _| d.is_open = true);
        let start = ws.docks.left.read(cx).size;
        ws.begin_dock_drag(layout::DockPosition::Left, 100.0, cx);
        ws.docks.left.update(cx, |d, _| d.resize(start + 30.0));
        assert_eq!(ws.docks.left.read(cx).size, start + 30.0);
        ws.docks.left.update(cx, |d, _| d.resize(99999.0));
        assert_eq!(ws.docks.left.read(cx).size, ws.docks.left.read(cx).max_size);
        ws.docks.left.update(cx, |d, _| d.resize(0.0));
        assert_eq!(ws.docks.left.read(cx).size, ws.docks.left.read(cx).min_size);
        ws.end_dock_drag(cx);
        assert!(ws.docks.drag.is_none());

        ws.docks.right.update(cx, |d, _| d.is_open = true);
        ws.docks.bottom.update(cx, |d, _| d.is_open = true);
        ws.begin_dock_drag(layout::DockPosition::Right, 0.0, cx);
        assert!(matches!(
            ws.docks.drag.map(|d| d.position),
            Some(layout::DockPosition::Right)
        ));
        ws.end_dock_drag(cx);
        ws.begin_dock_drag(layout::DockPosition::Bottom, 0.0, cx);
        assert!(matches!(
            ws.docks.drag.map(|d| d.position),
            Some(layout::DockPosition::Bottom)
        ));
        ws.end_dock_drag(cx);

        ws.begin_dock_drag(layout::DockPosition::Left, 100.0, cx);
        assert!(ws.docks.drag.is_some());
        ws.end_stale_resize_drags(cx);
        assert!(ws.docks.drag.is_none());
        ws.end_stale_resize_drags(cx);
        assert!(ws.docks.drag.is_none());
    });
}

// ---- Dock notify reentrancy ----
//
// Dock event listeners run while the Dock entity is leased, so any Workspace op
// they dispatch that reaches `notify_left_dock` / `notify_right_dock` must be
// lease-free — otherwise it double-leases the dock and aborts the app (a panic
// across the objc event boundary cannot unwind).

#[gpui::test]
fn notify_docks_safe_while_dock_is_leased(cx: &mut TestAppContext) {
    let (_wh, ws) = build_workspace(cx);
    let (left_dock, right_dock) =
        ws.read_with(cx, |ws, _| (ws.docks.left.clone(), ws.docks.right.clone()));

    left_dock.update(cx, |_, cx| {
        ws.update(cx, |ws, cx| {
            ws.set_left_dock_view(LeftDockView::Files, cx);
        });
    });

    right_dock.update(cx, |_, cx| {
        ws.update(cx, |ws, cx| {
            ws.set_right_dock_view(RightDockView::Skills, cx);
        });
    });
}

/// Every "show me this right panel" action opens the dock, not just the tab.
///
/// `set_right_dock_view` moves the tab selection and returns early when that
/// view is already selected, so an affordance built on it alone shows nothing
/// while the dock is closed — and closed is how it starts. Each action below
/// was written that way; the assertion that catches it is `is_open`, not the
/// view, which is why asserting the view is not enough on its own.
#[gpui::test]
fn every_right_panel_action_opens_the_dock_it_selects_in(cx: &mut TestAppContext) {
    use crate::workspace::pages::Page;
    use crate::workspace::{
        FocusSkillSearch, SwitchRightPanelFlows, SwitchRightPanelSkills, SwitchRightPanelTasks,
        SwitchRightPanelTools, SwitchRightPanelUsage,
    };

    let (wh, ws) = build_workspace(cx);
    cx.update_window(wh.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            // Each action is checked from a closed dock, or the assertion
            // below would hold for a handler that only ever moves the tab.
            let mut check =
                |view: RightDockView,
                 act: &mut dyn FnMut(&mut Workspace, &mut Context<Workspace>)| {
                    if ws_is_open(ws, cx) {
                        ws.docks.right.update(cx, |d, _| d.toggle());
                    }
                    assert!(!ws_is_open(ws, cx), "the fixture left the dock open");
                    act(ws, cx);
                    assert!(ws_is_open(ws, cx), "{view:?} is behind a closed dock");
                    assert_eq!(ws.docks.right_view, view);
                };

            check(RightDockView::Usage, &mut |ws, cx| {
                ws.on_switch_right_panel_usage(&SwitchRightPanelUsage, window, cx)
            });
            check(RightDockView::Skills, &mut |ws, cx| {
                ws.on_switch_right_panel_skills(&SwitchRightPanelSkills, window, cx)
            });
            check(RightDockView::Tools, &mut |ws, cx| {
                ws.on_switch_right_panel_tools(&SwitchRightPanelTools, window, cx)
            });
            // `Cmd+/` promises the query box, which is not rendered at all
            // while the dock is shut.
            check(RightDockView::Skills, &mut |ws, cx| {
                ws.on_focus_skill_search(&FocusSkillSearch, window, cx)
            });

            // Tasks and Flows are pages: they must leave the utility dock shut.
            ws.docks.right.update(cx, |d, _| d.toggle());
            assert!(!ws_is_open(ws, cx), "the fixture left the dock open");
            ws.on_switch_right_panel_tasks(&SwitchRightPanelTasks, window, cx);
            assert_eq!(ws.active_page(), Some(Page::Tasks));
            ws.on_switch_right_panel_flows(&SwitchRightPanelFlows, window, cx);
            assert_eq!(ws.active_page(), Some(Page::Flows));
            assert!(!ws_is_open(ws, cx), "pages do not open the utility dock");
        });
    })
    .expect("the test window is live");
}

fn ws_is_open(ws: &Workspace, cx: &gpui::App) -> bool {
    ws.docks.right.read(cx).is_open
}

#[gpui::test]
async fn collapsing_and_resetting_left_dock_preserves_the_active_page(cx: &mut TestAppContext) {
    let (wh, ws) = build_workspace(cx);
    cx.run_until_parked();
    cx.update_window(wh.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.open_page(crate::workspace::pages::Page::Tasks, window, cx);
            ws.on_toggle_left_dock(&crate::workspace::ToggleLeftDock, window, cx);
            assert_eq!(ws.active_page(), Some(crate::workspace::pages::Page::Tasks));
            assert_eq!(
                ws.docks.left.read(cx).display_size(),
                crate::ui::theme::DOCK_LEFT_RAIL_W
            );
            ws.on_reset_left_dock_width(&crate::workspace::ResetLeftDockWidth, window, cx);
            assert!(ws.docks.left.read(cx).is_open);
            assert_eq!(
                ws.docks.left.read(cx).size,
                crate::ui::theme::DOCK_LEFT_DEFAULT_W
            );
            assert_eq!(ws.active_page(), Some(crate::workspace::pages::Page::Tasks));
        })
    })
    .unwrap();
}

#[gpui::test]
async fn left_dock_drag_snaps_without_losing_the_expanded_width(cx: &mut TestAppContext) {
    let (wh, ws) = build_workspace(cx);
    cx.run_until_parked();
    cx.update_window(wh.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.docks.left.update(cx, |dock, _| {
                dock.is_open = true;
                dock.resize(321.0);
            });
            ws.begin_dock_drag(layout::DockPosition::Left, 321.0, cx);
            ws.update_dock_drag(40.0, window, cx);
            assert!(!ws.docks.left.read(cx).is_open);
            assert_eq!(ws.docks.left.read(cx).size, 321.0);
            ws.update_dock_drag(300.0, window, cx);
            assert!(ws.docks.left.read(cx).is_open);
            assert_eq!(ws.docks.left.read(cx).size, 300.0);
            ws.end_dock_drag(cx);
        })
    })
    .unwrap();
}

#[gpui::test]
fn bottom_input_font_reload_updates_only_its_snapshot_and_floor(cx: &mut TestAppContext) {
    let (_wh, ws) = build_workspace(cx);
    ws.update(cx, |ws, cx| {
        ws.activate_bottom_input(cx);
        ws.docks.bottom.update(cx, |dock, _| dock.resize(0.0));
        let original_chat_font = ws.mirrors.shared_surface.agent_chat_font.clone();
        let original_editor_font = ws.mirrors.shared_surface.editor_font.clone();
        let before = ws.bottom_dock_display_size(cx);
        let mut config = daruda_config::Config::default();
        config.font.input.size = 24.0;
        config.font.input.line_height = 1.5;
        ws.apply_config(&config, cx);
        let snapshot = ws.prepare_bottom_dock_snapshot(cx);
        assert_eq!(snapshot.input_font, config.font.input);
        assert_eq!(ws.input_dock.font.row_height(), 36.0);
        assert_eq!(ws.bottom_dock_display_size(cx) - before, 16.0);
        assert_eq!(
            ws.mirrors.shared_surface.agent_chat_font,
            original_chat_font
        );
        assert_eq!(ws.mirrors.shared_surface.editor_font, original_editor_font);
    });
}

#[gpui::test]
fn bottom_input_font_reload_refits_a_wrapped_draft(cx: &mut TestAppContext) {
    let (window_handle, ws) = build_workspace(cx);
    let draft = "draft ".repeat(25);
    cx.update_window(window_handle.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.activate_bottom_input(cx);
            ws.docks.bottom.update(cx, |dock, _| {
                dock.open();
                dock.resize(0.0);
            });
            ws.input_dock.input.update(cx, |state, cx| {
                state.set_value(draft.clone(), window, cx);
                state.focus(window, cx);
            });
        });
    })
    .unwrap();
    let mut vcx = gpui::VisualTestContext::from_window(window_handle.into(), cx);
    vcx.run_until_parked();
    let before = ws.read_with(&vcx, |ws, cx| ws.input_dock.input.read(cx).display_rows());
    let mut config = daruda_config::Config::default();
    config.font.input.size = 24.0;
    config.font.input.line_height = 1.5;
    ws.update(&mut vcx, |ws, cx| ws.apply_config(&config, cx));
    vcx.run_until_parked();
    vcx.update(|window, cx| {
        ws.read_with(cx, |ws, cx| {
            let state = ws.input_dock.input.read(cx);
            let rows = state
                .display_map()
                .wrap_row_count()
                .clamp(1, usize::from(config.agent.input_max_rows));
            assert!(rows > before, "larger text wraps to more rows");
            assert_eq!(state.display_rows(), rows);
            assert_eq!(ws.input_dock.line_count, rows);
            assert_eq!(state.value().as_ref(), draft);
            assert!(state.focus_handle(cx).is_focused(window));
        });
    });
}

#[gpui::test]
fn large_input_font_caps_rows_to_the_bottom_dock_height(cx: &mut TestAppContext) {
    let mut config = daruda_config::Config::default();
    config.font.input.size = 72.0;
    config.font.input.line_height = 2.0;
    let (window, ws) = build_workspace_with(cx, &config, None);
    cx.update_window(window.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| {
            ws.activate_bottom_input(cx);
            ws.docks.bottom.update(cx, |dock, _| dock.resize(0.0));
            ws.input_dock.input.update(cx, |state, cx| {
                state.set_value("first\nsecond\nthird", window, cx);
            });
            ws.adapt_dock_to_input_lines(window, cx);
            assert_eq!(ws.input_dock.input.read(cx).display_rows(), 2);
            assert_eq!(ws.input_dock.line_count, 2);
            assert_eq!(
                ws.bottom_dock_display_size(cx),
                layout::ops::bottom_dock_height_for_rows(2, 144.0)
            );
            ws.set_composer_expanded(true, window, cx);
            assert_eq!(ws.input_dock.input.read(cx).display_rows(), 2);
        });
    })
    .unwrap();
}

#[gpui::test]
fn large_input_font_keeps_send_visible_as_queue_changes(cx: &mut TestAppContext) {
    let mut config = daruda_config::Config::default();
    config.font.input.size = 72.0;
    config.font.input.line_height = 2.0;
    let (handle, ws) = build_workspace_with(cx, &config, None);
    let (pane_id, view) = cx
        .update_window(handle.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| {
                let pane = ws.create_agent_chat_pane(
                    None,
                    None,
                    daruda_config::AgentDefinition::claude_default().id,
                    None,
                    window,
                    cx,
                );
                let id = pane.id;
                let view = pane.agent_chat_view().unwrap().clone();
                ws.active_runtime_mut().panes.push(pane);
                ws.active_runtime_mut().focused_pane_id = id;
                ws.activate_bottom_input(cx);
                ws.docks.bottom.update(cx, |dock, _| {
                    dock.open();
                    dock.resize(0.0);
                });
                ws.input_dock.input.update(cx, |state, cx| {
                    state.set_value("first\nsecond\nthird", window, cx);
                    state.focus(window, cx);
                });
                (id, view)
            })
        })
        .unwrap();
    let mut vcx = gpui::VisualTestContext::from_window(handle.into(), cx);
    vcx.run_until_parked();
    let before = ws.read_with(&vcx, |ws, _| ws.input_dock.row_limit);
    let saved_size = ws.read_with(&vcx, |ws, cx| ws.docks.bottom.read(cx).size);
    assert_eq!(before, 2);
    view.update(&mut vcx, |view, cx| {
        view.fill_queue_for_test(1);
        cx.notify();
    });
    vcx.run_until_parked();
    let button = vcx
        .debug_bounds("composer-submit")
        .expect("send button painted");
    let body = vcx
        .debug_bounds("bottom-input-body")
        .expect("input body painted");
    vcx.update(|window, cx| {
        ws.read_with(cx, |ws, cx| {
            let state = ws.input_dock.input.read(cx);
            assert_eq!(ws.input_dock.row_limit, 1);
            assert_eq!(ws.docks.bottom.read(cx).size, saved_size);
            assert_eq!(ws.input_dock.line_count, 1);
            assert_eq!(state.display_rows(), 1);
            assert_eq!(state.value().as_ref(), "first\nsecond\nthird");
            assert!(state.focus_handle(cx).is_focused(window));
            assert!(state.input_bounds().bottom() <= button.top());
            assert!(button.bottom() <= body.bottom());
            assert!(ws.bottom_dock_display_size(cx) <= crate::ui::theme::DOCK_BOTTOM_MAX_H);
        });
        ws.update(cx, |ws, cx| ws.clear_queued_prompts(pane_id, window, cx));
    });
    vcx.run_until_parked();
    ws.read_with(&vcx, |ws, cx| {
        assert_eq!(ws.input_dock.row_limit, before);
        assert_eq!(ws.docks.bottom.read(cx).size, saved_size);
        assert_eq!(ws.input_dock.input.read(cx).display_rows(), 2);
    });
}
