//! A worktree's side tabs: a file tab sent beside the others stays in view
//! while they change, takes the keyboard without becoming the focused pane,
//! and comes back or closes on its own.

use super::*;
use crate::workspace::main_area::side_group::GroupsShown;
use crate::workspace::main_area::tab_ops::OpenIntent;

type Fixture = (
    gpui::WindowHandle<Workspace>,
    gpui::Entity<Workspace>,
    tempfile::TempDir,
);

/// A workspace whose worktree has its first tab and `names` open as file
/// tabs after it, wide enough for both groups.
fn with_files(cx: &mut TestAppContext, names: &[&str]) -> Fixture {
    let (wh, ws, temp) = build_workspace_with_temp_project(cx);
    cx.update(|cx| {
        crate::window_registry::WindowRegistry::register(wh.into(), ws.downgrade(), cx);
    });
    let id = ws.read_with(cx, |ws, _| ws.active_ref());
    // These files open among the tabs; each test sends aside what it needs.
    ws.update(cx, |ws, _| ws.mirrors.file_viewer_open_beside = false);
    // The fixture opens no tab of its own.
    if ws.read_with(cx, |ws, _| ws.active_runtime().tabs.is_empty()) {
        cx.update_window(wh.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| ws.add_tab(window, cx));
        })
        .unwrap();
    }
    for name in names {
        cx.update_window(wh.into(), |_, window, cx| {
            ws.update(cx, |ws, cx| {
                ws.open_files_entry(id, name.into(), OpenIntent::Enter, window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
    }
    ws.update(cx, |ws, _| ws.main_area.groups_width = 1200.0);
    (wh, ws, temp)
}

fn act(
    f: &Fixture,
    cx: &mut TestAppContext,
    op: impl FnOnce(&mut Workspace, &mut gpui::Window, &mut gpui::Context<Workspace>),
) {
    let ws = f.1.clone();
    cx.update_window(f.0.into(), |_, window, cx| {
        ws.update(cx, |ws, cx| op(ws, window, cx))
    })
    .unwrap();
    cx.run_until_parked();
}

/// The paths the side tabs show, in order, by file name.
fn side_files(f: &Fixture, cx: &mut TestAppContext) -> Vec<String> {
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        runtime
            .side
            .tabs()
            .iter()
            .filter_map(|tab| runtime.panes.iter().find(|pane| pane.id == tab.pane))
            .filter_map(|pane| pane.file_path())
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    })
}

#[gpui::test]
async fn a_file_tab_goes_aside_and_the_tabs_keep_their_own_focus(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    let (tabs_before, shell) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (runtime.tabs.len(), runtime.tabs[0].last_focused_pane)
    });
    // A shell is not read beside a chat: it stays where it is.
    f.1.read_with(cx, |ws, _| {
        assert!(!ws.tab_can_go_aside(0));
        assert!(ws.tab_can_go_aside(1));
    });
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(0, window, cx));
    assert!(side_files(&f, cx).is_empty());

    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    assert_eq!(side_files(&f, cx), ["a.txt"]);
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert_eq!(runtime.tabs.len(), tabs_before - 1);
        // The keyboard is aside; the focused pane is still one of the tabs'.
        assert!(ws.keyboard_is_aside());
        assert_eq!(runtime.focused_pane_id, shell);
        assert_eq!(ws.keyboard_pane(), runtime.side.active_pane().unwrap());
        assert!(matches!(ws.groups_shown(), GroupsShown::Both { .. }));
        // Both are drawn.
        assert!(ws.pane_on_screen(shell));
        assert!(ws.pane_on_screen(runtime.side.active_pane().unwrap()));
        // What the file commands act on is the file aside.
        assert!(ws.focused_file_view().is_some());
    });
}

#[gpui::test]
async fn pressing_a_pane_moves_the_keyboard_between_the_groups(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let (shell, aside) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (runtime.focused_pane_id, runtime.side.active_pane().unwrap())
    });

    act(&f, cx, |ws, window, cx| {
        ws.focus_pane_on_click(shell, window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        assert!(!ws.keyboard_is_aside());
        assert!(ws.focused_file_view().is_none());
    });

    act(&f, cx, |ws, window, cx| {
        ws.focus_pane_on_click(aside, window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert!(ws.keyboard_is_aside());
        assert_eq!(runtime.focused_pane_id, shell, "never a side pane");
    });

    act(&f, cx, |ws, window, cx| {
        ws.focus_other_tab_group(window, cx)
    });
    f.1.read_with(cx, |ws, _| assert!(!ws.keyboard_is_aside()));
    act(&f, cx, |ws, window, cx| {
        ws.focus_other_tab_group(window, cx)
    });
    f.1.read_with(cx, |ws, _| assert!(ws.keyboard_is_aside()));
}

#[gpui::test]
async fn tab_commands_follow_the_keyboard_into_the_side_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(2, window, cx));
    act(&f, cx, |ws, window, cx| ws.focus_tabs_group(window, cx));
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    assert_eq!(side_files(&f, cx), ["b.txt", "a.txt"]);

    // Next Tab wraps inside the side tabs; the tabs on the left stay.
    let left =
        f.1.read_with(cx, |ws, _| ws.active_runtime().active_tab_index);
    act(&f, cx, |ws, window, cx| {
        ws.on_next_tab(&crate::workspace::NextTab, window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert_eq!(runtime.side.active_index(), 0);
        assert_eq!(runtime.active_tab_index, left);
    });

    // Move Tab Right reorders the side tabs.
    act(&f, cx, |ws, window, cx| {
        ws.on_move_tab_right(&crate::workspace::MoveTabRight, window, cx)
    });
    assert_eq!(side_files(&f, cx), ["a.txt", "b.txt"]);

    // Close Tab closes the side tab shown, not a tab on the left.
    let tabs = f.1.read_with(cx, |ws, _| ws.active_runtime().tabs.len());
    act(&f, cx, |ws, window, cx| {
        ws.on_close_tab(&crate::workspace::CloseTab, window, cx)
    });
    assert_eq!(side_files(&f, cx), ["a.txt"]);
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert_eq!(runtime.tabs.len(), tabs);
        assert_eq!(runtime.panes.len(), tabs + 1, "the closed pane is gone");
        assert!(ws.keyboard_is_aside(), "the keyboard stays aside");
    });
}

#[gpui::test]
async fn the_last_side_tab_closed_gives_the_width_and_the_keyboard_back(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| {
        ws.request_close_side_tab(0, window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert!(runtime.side.is_empty());
        assert!(!ws.keyboard_is_aside());
        assert_eq!(ws.groups_shown(), GroupsShown::Tabs);
        assert_eq!(ws.keyboard_pane(), runtime.focused_pane_id);
    });
}

#[gpui::test]
async fn a_side_tab_comes_back_among_the_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let aside =
        f.1.read_with(cx, |ws, _| ws.active_runtime().side.active_pane().unwrap());
    act(&f, cx, |ws, window, cx| {
        ws.move_tab_to_other_group(window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert!(runtime.side.is_empty());
        assert_eq!(runtime.tabs.len(), 2);
        assert_eq!(runtime.active_tab_index, 1);
        assert_eq!(runtime.focused_pane_id, aside);
    });
}

#[gpui::test]
async fn the_last_tab_closed_leaves_the_side_tabs_open(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| ws.close_tab_at(0, window, cx));
    assert_eq!(side_files(&f, cx), ["a.txt"]);
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert!(runtime.tabs.is_empty());
        assert_eq!(runtime.panes.len(), 1);
        assert!(ws.keyboard_is_aside(), "the keyboard has nowhere else");
        assert!(!ws.has_focused_pane());
    });
    // With no side tab either, the worktree is emptied as before.
    act(&f, cx, |ws, window, cx| {
        ws.request_close_side_tab(0, window, cx)
    });
    f.1.read_with(cx, |ws, _| assert!(ws.active_runtime().panes.is_empty()));
}

#[gpui::test]
async fn hidden_side_tabs_stay_open_and_a_zoom_or_a_page_takes_the_whole_area(
    cx: &mut TestAppContext,
) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let (shell, aside) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (runtime.focused_pane_id, runtime.side.active_pane().unwrap())
    });

    act(&f, cx, |ws, window, cx| ws.toggle_side_group(window, cx));
    f.1.read_with(cx, |ws, _| {
        assert_eq!(ws.groups_shown(), GroupsShown::Tabs);
        assert!(ws.side_tabs_hidden());
        assert!(!ws.keyboard_is_aside());
        assert!(!ws.pane_on_screen(aside));
    });
    act(&f, cx, |ws, window, cx| ws.toggle_side_group(window, cx));
    f.1.read_with(cx, |ws, _| assert!(ws.pane_on_screen(aside)));

    // No room for both: the group the keyboard is in. Moving it measures
    // the test window again, so the width is narrowed after each move.
    let narrow = |f: &Fixture, cx: &mut TestAppContext| {
        f.1.update(cx, |ws, _| ws.main_area.groups_width = 400.0)
    };
    act(&f, cx, |ws, window, cx| ws.focus_side_group(window, cx));
    narrow(&f, cx);
    f.1.read_with(cx, |ws, _| {
        assert_eq!(ws.groups_shown(), GroupsShown::Side);
        assert!(!ws.pane_on_screen(shell));
    });
    act(&f, cx, |ws, window, cx| ws.focus_tabs_group(window, cx));
    narrow(&f, cx);
    f.1.read_with(cx, |ws, _| assert_eq!(ws.groups_shown(), GroupsShown::Tabs));

    f.1.update(cx, |ws, _| {
        ws.main_area.groups_width = 1200.0;
        ws.main_area.zoomed_pane_id = Some(shell);
    });
    f.1.read_with(cx, |ws, _| assert_eq!(ws.groups_shown(), GroupsShown::Tabs));
}

#[gpui::test]
async fn dragging_the_divider_left_widens_the_side_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let before = f.1.read_with(cx, |ws, _| ws.active_runtime().side.ratio());
    act(&f, cx, |ws, window, cx| {
        assert!(!ws.drag_side_divider(500.0, window, cx), "nothing held yet");
        ws.begin_side_divider_drag(600.0, cx);
        // `resize_all_tabs` measures the test window; keep the width asked for.
        assert!(ws.drag_side_divider(480.0, window, cx));
        ws.end_side_divider_drag(cx);
    });
    f.1.read_with(cx, |ws, _| {
        assert!(ws.active_runtime().side.ratio() > before);
        assert!(ws.main_area.side_divider_drag.is_none());
    });
}

/// What the file commands act on is the pane the keyboard is in, and what
/// splits, zooms and takes the bottom input is the focused pane.
#[gpui::test]
async fn file_commands_act_on_the_file_the_keyboard_is_in(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    // b.txt aside with the keyboard in it; a.txt the focused pane on the left.
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(2, window, cx));
    let (left, aside) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (runtime.focused_pane_id, runtime.side.active_pane().unwrap())
    });
    let name = |ws: &Workspace| {
        ws.focused_file_view()
            .and_then(|view| view.path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    };
    f.1.read_with(cx, |ws, _| {
        assert_eq!(name(ws).as_deref(), Some("b.txt"));
        assert_eq!(ws.keyboard_pane(), aside);
    });

    // Escape closes the file the keyboard is in, not the one on the left.
    act(&f, cx, |ws, window, cx| {
        ws.close_focused_file_pane(window, cx)
    });
    assert!(side_files(&f, cx).is_empty());
    f.1.read_with(cx, |ws, _| {
        assert_eq!(ws.active_runtime().focused_pane_id, left);
        assert_eq!(name(ws).as_deref(), Some("a.txt"));
    });
}

#[gpui::test]
async fn a_zoomed_pane_takes_the_file_commands_from_the_hidden_side_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let (left, aside) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (runtime.focused_pane_id, runtime.side.active_pane().unwrap())
    });
    f.1.update(cx, |ws, _| ws.main_area.zoomed_pane_id = Some(left));
    f.1.read_with(cx, |ws, _| {
        assert!(!ws.keyboard_is_aside());
        assert_eq!(ws.keyboard_pane(), left);
    });
    // Unzoomed, the keyboard is where it was left.
    f.1.update(cx, |ws, _| ws.main_area.zoomed_pane_id = None);
    f.1.read_with(cx, |ws, _| assert_eq!(ws.keyboard_pane(), aside));
}

#[gpui::test]
async fn a_file_open_aside_is_not_opened_again_among_the_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(2, window, cx));
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| ws.focus_tabs_group(window, cx));
    let (lane, panes, tabs) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (ws.active_ref(), runtime.panes.len(), runtime.tabs.len())
    });

    // A row the cursor rests on shows it there and leaves the keyboard.
    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "b.txt".into(), OpenIntent::Preview, window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert_eq!((runtime.panes.len(), runtime.tabs.len()), (panes, tabs));
        assert_eq!(runtime.side.active_index(), 0);
        assert!(!ws.keyboard_is_aside());
    });
    // Entering it moves the keyboard there.
    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "a.txt".into(), OpenIntent::Enter, window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert_eq!((runtime.panes.len(), runtime.tabs.len()), (panes, tabs));
        assert_eq!(runtime.side.active_index(), 1);
        assert!(ws.keyboard_is_aside());
    });
}

/// With no tab left the focused pane is none, whatever id a side pane has:
/// the first pane a window makes is numbered like the "none" value.
#[gpui::test]
async fn a_side_pane_is_never_taken_for_the_focused_one(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| ws.close_tab_at(0, window, cx));
    // Make the "none" value name the side pane, as in a fresh window.
    f.1.update(cx, |ws, _| {
        let runtime = ws.active_runtime_mut();
        let aside = runtime.side.active_pane().unwrap();
        runtime.focused_pane_id = aside;
    });
    f.1.read_with(cx, |ws, _| assert!(!ws.has_focused_pane()));
    act(&f, cx, |ws, window, cx| {
        ws.focus_other_tab_group(window, cx)
    });
    f.1.read_with(cx, |ws, _| {
        assert!(!ws.keyboard_is_aside(), "it can be left")
    });
    act(&f, cx, |ws, window, cx| {
        ws.focus_other_tab_group(window, cx)
    });
    f.1.read_with(cx, |ws, _| assert!(ws.keyboard_is_aside()));
}

#[gpui::test]
async fn side_tabs_shown_again_come_into_view_where_only_one_group_fits(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| ws.toggle_side_group(window, cx));
    act(&f, cx, |ws, window, cx| ws.toggle_side_group(window, cx));
    f.1.update(cx, |ws, _| ws.main_area.groups_width = 400.0);
    f.1.read_with(cx, |ws, _| {
        assert!(ws.keyboard_is_aside());
        assert_eq!(ws.groups_shown(), GroupsShown::Side);
    });
}

#[gpui::test]
async fn a_divider_held_does_not_follow_to_another_worktree(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let before = f.1.read_with(cx, |ws, _| ws.active_runtime().side.ratio());
    act(&f, cx, |ws, window, cx| {
        ws.begin_side_divider_drag(600.0, cx);
        // The worktree on screen changes under the pointer.
        let other = daruda_store::project::LaneRef {
            lane: ws.active.lane + 1,
            ..ws.active
        };
        ws.main_area.runtimes.entry(other).or_default();
        ws.active = other;
        assert!(!ws.drag_side_divider(300.0, window, cx));
        assert!(ws.main_area.side_divider_drag.is_none());
    });
    let first = f.1.read_with(cx, |ws, _| {
        let lane = daruda_store::project::LaneRef {
            lane: ws.active.lane - 1,
            ..ws.active
        };
        ws.main_area.runtimes[&lane].side.ratio()
    });
    assert_eq!(first, before);
}

/// Side tabs, the one shown, their share and whether they are hidden come
/// back after a restart; a saved side tab that is not a file does not.
#[gpui::test]
async fn side_tabs_are_saved_and_restored(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(2, window, cx));
    act(&f, cx, |ws, window, cx| ws.focus_tabs_group(window, cx));
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| {
        ws.activate_side_tab(0, window, cx);
        ws.active_runtime_mut().side.set_ratio(0.3);
        ws.toggle_side_group(window, cx);
    });
    let (workspace_state, mut project_states) =
        f.1.read_with(cx, |ws, cx| ws.snapshot_for_disk(cx));
    let lane = &mut project_states[0].lanes[0];
    let side = lane.side.as_mut().expect("side tabs saved");
    assert_eq!((side.tabs.len(), side.active, side.collapsed), (2, 0, true));
    assert_eq!(side.ratio, 0.3);
    // A shell saved as a side tab (a file written by hand): dropped.
    side.tabs.insert(0, lane.tabs[0].clone());
    side.active = 2;

    let config = daruda_config::Config::default();
    let restored = cx.add_window(|window, cx| {
        let mut ws =
            Workspace::new_with_project_for_test(&config, None, fresh_test_data_dir(), window, cx);
        ws.restore_from_disk(&workspace_state, &project_states, window, cx);
        ws
    });
    let restored = (
        restored,
        restored.root(cx).unwrap(),
        tempfile::tempdir().unwrap(),
    );
    cx.run_until_parked();
    assert_eq!(side_files(&restored, cx), ["b.txt", "a.txt"]);
    restored.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        // The index counted the dropped tab; it still names a.txt.
        assert_eq!(runtime.side.active_index(), 1);
        assert_eq!(runtime.side.ratio(), 0.3);
        assert!(runtime.side.is_collapsed());
        assert_eq!(runtime.tabs.len(), 1);
        assert_eq!(runtime.panes.len(), 3);
        assert!(!ws.keyboard_is_aside());
        assert!(ws.has_focused_pane());
    });
}

/// Where a file opens: beside the tabs by default, among them when the
/// setting says so or there is no tab to be beside, and wherever the
/// keyboard already is.
#[gpui::test]
async fn a_file_opens_beside_the_tabs_unless_told_or_alone(cx: &mut TestAppContext) {
    // (label, the setting, a tab on the left, keyboard aside first, opens aside)
    let cases = [
        ("the default", true, true, false, true),
        ("the setting off", false, true, false, false),
        ("no tab to be beside", true, false, false, false),
        ("the keyboard already aside", false, true, true, true),
    ];
    for (label, setting, left_tab, keyboard_aside, aside) in cases {
        let f = with_files(cx, &[]);
        let lane = f.1.read_with(cx, |ws, _| ws.active_ref());
        if keyboard_aside {
            act(&f, cx, |ws, window, cx| {
                ws.open_files_entry(lane, "b.txt".into(), OpenIntent::Enter, window, cx);
                ws.send_tab_aside(1, window, cx);
            });
        }
        if !left_tab {
            act(&f, cx, |ws, window, cx| ws.close_tab_at(0, window, cx));
        }
        f.1.update(cx, |ws, _| ws.mirrors.file_viewer_open_beside = setting);
        act(&f, cx, |ws, window, cx| {
            ws.open_files_entry(lane, "a.txt".into(), OpenIntent::Enter, window, cx)
        });
        let opened_aside = side_files(&f, cx).contains(&"a.txt".to_owned());
        assert_eq!(opened_aside, aside, "{label}");
        f.1.read_with(cx, |ws, _| {
            assert_eq!(
                ws.keyboard_is_aside(),
                aside,
                "{label}: entered where it opened"
            );
            assert_eq!(
                ws.focused_file_view()
                    .and_then(|view| view.path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .as_deref(),
                Some("a.txt"),
                "{label}"
            );
        });
    }
}

/// Skimming rows in a panel reuses one side tab and leaves the keyboard in
/// the panel; a row entered keeps its tab, and the next skim opens another.
#[gpui::test]
async fn skimming_reuses_one_side_tab_until_a_file_is_entered(cx: &mut TestAppContext) {
    let f = with_files(cx, &[]);
    let lane = f.1.read_with(cx, |ws, _| ws.active_ref());
    f.1.update(cx, |ws, _| ws.mirrors.file_viewer_open_beside = true);
    let open = |f: &Fixture, cx: &mut TestAppContext, name: &'static str, intent| {
        act(f, cx, move |ws, window, cx| {
            ws.open_files_entry(lane, name.into(), intent, window, cx)
        });
    };

    open(&f, cx, "a.txt", OpenIntent::Preview);
    open(&f, cx, "b.txt", OpenIntent::Preview);
    assert_eq!(side_files(&f, cx), ["b.txt"]);
    f.1.read_with(cx, |ws, cx| {
        assert!(!ws.keyboard_is_aside(), "a skim stays in the panel");
        assert_eq!(ws.preview_tab_index(cx), None, "the slot is not a tab's");
        assert!(matches!(ws.groups_shown(), GroupsShown::Both { .. }));
    });

    open(&f, cx, "b.txt", OpenIntent::Enter);
    f.1.read_with(cx, |ws, cx| {
        assert!(ws.keyboard_is_aside());
        assert_eq!(
            ws.preview_slot(cx),
            None,
            "entered: no longer the scratch tab"
        );
    });
    open(&f, cx, "a.txt", OpenIntent::Preview);
    assert_eq!(side_files(&f, cx), ["b.txt", "a.txt"]);
}

/// A panel row opened beside the tabs keeps the keyboard in the panel, is
/// still the file the row stands for, and a second Enter walks into it.
#[gpui::test]
async fn a_row_opened_beside_the_tabs_is_entered_by_a_second_enter(cx: &mut TestAppContext) {
    use crate::workspace::main_area::file_view_pane::DiffSource;
    let f = with_files(cx, &[]);
    let lane = f.1.read_with(cx, |ws, _| ws.active_ref());
    f.1.update(cx, |ws, _| ws.mirrors.file_viewer_open_beside = true);
    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "a.txt".into(), OpenIntent::Commit, window, cx)
    });
    let path = f.1.read_with(cx, |ws, _| {
        assert!(!ws.keyboard_is_aside());
        assert!(
            ws.focused_file_view().is_none(),
            "the keyboard is not in it"
        );
        let (_, view) = ws.shown_file_view().expect("it is the file on show");
        view.path.clone()
    });
    act(&f, cx, move |ws, window, cx| {
        assert!(ws.step_into_open_file_view(Some(path), DiffSource::WorkingTree, window, cx));
    });
    f.1.read_with(cx, |ws, _| assert!(ws.keyboard_is_aside()));
}

/// Where only one group fits, a row skimmed in a panel opens among the
/// tabs, which are what is drawn; entered, it opens aside and is shown.
#[gpui::test]
async fn a_skim_in_a_narrow_window_opens_where_it_is_seen(cx: &mut TestAppContext) {
    let f = with_files(cx, &[]);
    let lane = f.1.read_with(cx, |ws, _| ws.active_ref());
    f.1.update(cx, |ws, _| {
        ws.mirrors.file_viewer_open_beside = true;
        ws.main_area.groups_width = 400.0;
    });
    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "a.txt".into(), OpenIntent::Preview, window, cx);
        ws.main_area.groups_width = 400.0;
    });
    assert!(side_files(&f, cx).is_empty());
    f.1.read_with(cx, |ws, _| assert_eq!(ws.active_runtime().tabs.len(), 2));

    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "b.txt".into(), OpenIntent::Enter, window, cx);
        ws.main_area.groups_width = 400.0;
    });
    assert_eq!(side_files(&f, cx), ["b.txt"]);
    f.1.read_with(cx, |ws, _| assert_eq!(ws.groups_shown(), GroupsShown::Side));
}

/// One preview at a time: a skim whose scratch tab is in the other group
/// closes that tab instead of leaving it behind as a tab nobody kept.
#[gpui::test]
async fn a_skim_takes_its_scratch_tab_with_it_to_the_other_group(cx: &mut TestAppContext) {
    let f = with_files(cx, &[]);
    let lane = f.1.read_with(cx, |ws, _| ws.active_ref());
    // Among the tabs first.
    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "a.txt".into(), OpenIntent::Preview, window, cx)
    });
    f.1.read_with(cx, |ws, cx| {
        assert_eq!(ws.active_runtime().tabs.len(), 2);
        assert_eq!(ws.preview_tab_index(cx), Some(1));
    });
    // Then files open beside the tabs.
    f.1.update(cx, |ws, _| {
        ws.mirrors.file_viewer_open_beside = true;
        ws.main_area.groups_width = 1200.0;
    });
    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(lane, "b.txt".into(), OpenIntent::Preview, window, cx)
    });
    assert_eq!(side_files(&f, cx), ["b.txt"]);
    f.1.read_with(cx, |ws, cx| {
        let runtime = ws.active_runtime();
        assert_eq!(runtime.tabs.len(), 1, "the scratch tab went with the skim");
        assert_eq!(runtime.panes.len(), 2);
        assert!(matches!(
            ws.preview_slot(cx),
            Some((
                crate::workspace::main_area::side_group::PanePlace::Side(0),
                _
            ))
        ));
    });
    // A file entered among the tabs is kept when the next skim goes aside.
    act(&f, cx, |ws, window, cx| {
        ws.mirrors.file_viewer_open_beside = false;
        ws.focus_tabs_group(window, cx);
        ws.open_files_entry(lane, "sub/nested.txt".into(), OpenIntent::Enter, window, cx);
    });
    f.1.read_with(cx, |ws, _| assert_eq!(ws.active_runtime().tabs.len(), 2));
}

/// Closing the scratch tab left among the tabs brings the keyboard back to
/// them, so the skim that closed it is placed by where the keyboard then
/// is, not by where it was when the row was pressed.
#[gpui::test]
async fn a_skim_is_placed_after_its_scratch_tab_is_closed(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    let lane = f.1.read_with(cx, |ws, _| ws.active_ref());
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    // A scratch tab among the tabs, then the keyboard back aside.
    act(&f, cx, |ws, window, cx| {
        ws.focus_tabs_group(window, cx);
        ws.open_files_entry(lane, "b.txt".into(), OpenIntent::Preview, window, cx);
    });
    act(&f, cx, |ws, window, cx| {
        ws.main_area.groups_width = 1200.0;
        ws.focus_side_group(window, cx);
    });
    f.1.read_with(cx, |ws, cx| {
        assert!(ws.keyboard_is_aside());
        assert_eq!(ws.preview_tab_index(cx), Some(1));
    });

    act(&f, cx, |ws, window, cx| {
        ws.open_files_entry(
            lane,
            "sub/nested.txt".into(),
            OpenIntent::Preview,
            window,
            cx,
        )
    });
    assert_eq!(side_files(&f, cx), ["a.txt"], "not opened aside");
    f.1.read_with(cx, |ws, cx| {
        assert!(!ws.keyboard_is_aside());
        assert_eq!(
            ws.active_runtime().tabs.len(),
            2,
            "b.txt's tab was replaced"
        );
        assert_eq!(ws.preview_tab_index(cx), Some(1));
    });
}
