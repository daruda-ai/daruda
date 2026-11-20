//! Dragging a tab within the side tabs and between them and the tabs: where
//! it lands follows the slot of the strip it is let go on, and a tab the
//! other strip does not take stays where it is.

use super::side_tabs::{Fixture, act, side_files, with_files};
use super::*;
use crate::workspace::main_area::pane_tree::DropHalf;
use crate::workspace::main_area::tab_drop_plan::{DropSlot, Strip};
use crate::workspace::main_area::tab_strip::TabId;

/// The files the tabs on the left hold, in order; a tab without one is "-".
fn tab_files(f: &Fixture, cx: &mut TestAppContext) -> Vec<String> {
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        runtime
            .tabs
            .iter()
            .map(|tab| {
                runtime
                    .panes
                    .iter()
                    .find(|pane| pane.id == tab.layout.first_leaf())
                    .and_then(|pane| pane.file_path())
                    .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                    .unwrap_or_else(|| "-".into())
            })
            .collect()
    })
}

fn tab_id(f: &Fixture, cx: &mut TestAppContext, strip: Strip, index: usize) -> TabId {
    f.1.read_with(cx, |ws, _| match strip {
        Strip::Tabs => ws.active_runtime().tabs[index].id,
        Strip::Side => ws.active_runtime().side.tabs()[index].id,
    })
}

/// Hold `dragged` over the slot before `index` of `strip`, as the cell
/// there would claim it, then let go on that strip.
fn drop_at_slot(f: &Fixture, cx: &mut TestAppContext, dragged: TabId, strip: Strip, index: usize) {
    act(f, cx, |ws, window, cx| {
        let facts = ws.dragged_tab(dragged);
        // Any cell's id tags the slot; the strip's first stands in.
        ws.claim_tab_drop_slot(strip, dragged, index, facts, cx);
        ws.drop_tab_onto_strip(strip, dragged, window, cx);
    });
}

#[gpui::test]
async fn a_side_tab_is_dragged_to_another_place_among_them(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    assert_eq!(side_files(&f, cx), ["a.txt", "b.txt"]);

    let b = tab_id(&f, cx, Strip::Side, 1);
    drop_at_slot(&f, cx, b, Strip::Side, 0);
    assert_eq!(side_files(&f, cx), ["b.txt", "a.txt"]);
    f.1.read_with(cx, |ws, _| {
        assert_eq!(
            ws.active_runtime().side.active_tab().map(|tab| tab.id),
            Some(b)
        );
        assert!(ws.main_area.tab_drag.slot.is_none(), "the drag is over");
    });

    // Its own slot and the one after it are where it is: nothing is drawn.
    for slot in [0, 1] {
        act(&f, cx, |ws, _, cx| {
            let facts = ws.dragged_tab(b);
            ws.claim_tab_drop_slot(Strip::Side, b, slot, facts, cx);
            assert!(ws.main_area.tab_drag.slot.is_none(), "slot {slot}");
        });
    }
}

#[gpui::test]
async fn a_file_tab_is_dragged_aside_and_a_shell_is_not(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    assert_eq!(tab_files(&f, cx), ["-", "b.txt"]);

    let b = tab_id(&f, cx, Strip::Tabs, 1);
    drop_at_slot(&f, cx, b, Strip::Side, 0);
    assert_eq!(side_files(&f, cx), ["b.txt", "a.txt"]);
    assert_eq!(tab_files(&f, cx), ["-"]);
    f.1.read_with(cx, |ws, _| assert!(ws.keyboard_is_aside()));

    // The shell: no slot of the side tabs is claimed, and letting go there
    // leaves it among the tabs.
    let shell = tab_id(&f, cx, Strip::Tabs, 0);
    drop_at_slot(&f, cx, shell, Strip::Side, 1);
    assert_eq!(side_files(&f, cx), ["b.txt", "a.txt"]);
    assert_eq!(tab_files(&f, cx), ["-"]);
    act(&f, cx, |ws, window, cx| {
        ws.drop_tab_onto_side_content(shell, window, cx)
    });
    assert_eq!(tab_files(&f, cx), ["-"]);
}

#[gpui::test]
async fn a_side_tab_is_dragged_back_to_a_place_among_the_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    assert_eq!(tab_files(&f, cx), ["-", "b.txt"]);

    let a = tab_id(&f, cx, Strip::Side, 0);
    drop_at_slot(&f, cx, a, Strip::Tabs, 1);
    assert_eq!(tab_files(&f, cx), ["-", "a.txt", "b.txt"]);
    assert!(side_files(&f, cx).is_empty());
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert_eq!(runtime.tabs.active_tab().map(|tab| tab.id), Some(a));
        assert_eq!(runtime.focused_pane_id, runtime.tabs[1].last_focused_pane);
        assert!(!ws.keyboard_is_aside());
    });
}

/// A slot the other strip's cells left set is not this strip's: the tab
/// goes after its last tab.
#[gpui::test]
async fn a_slot_of_the_other_strip_does_not_place_the_drop(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let b = tab_id(&f, cx, Strip::Tabs, 1);
    let shell = tab_id(&f, cx, Strip::Tabs, 0);
    act(&f, cx, |ws, window, cx| {
        ws.main_area.tab_drag.slot = Some(DropSlot {
            strip: Strip::Tabs,
            owner: shell,
            index: 0,
        });
        ws.drop_tab_onto_strip(Strip::Side, b, window, cx);
    });
    assert_eq!(side_files(&f, cx), ["a.txt", "b.txt"]);
    assert_eq!(tab_files(&f, cx), ["-"]);
}

#[gpui::test]
async fn a_file_tab_let_go_on_what_is_read_aside_joins_it(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt", "b.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let b = tab_id(&f, cx, Strip::Tabs, 1);
    let shown_aside =
        f.1.read_with(cx, |ws, _| ws.active_runtime().side.shown_pane().unwrap());
    act(&f, cx, |ws, window, cx| {
        ws.drop_tab_onto_pane(b, shown_aside, window, cx)
    });
    assert_eq!(side_files(&f, cx), ["a.txt", "b.txt"]);
    assert_eq!(tab_files(&f, cx), ["-"]);
}

#[gpui::test]
async fn a_side_file_is_dragged_into_a_pane_of_the_tabs(cx: &mut TestAppContext) {
    let f = with_files(cx, &["a.txt"]);
    act(&f, cx, |ws, window, cx| ws.send_tab_aside(1, window, cx));
    let a = tab_id(&f, cx, Strip::Side, 0);
    let (shell, file_pane) = f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        (runtime.focused_pane_id, runtime.side.tabs()[0].pane)
    });
    act(&f, cx, |ws, window, cx| {
        ws.main_area.pane_drop_hover = Some((shell, DropHalf::East));
        ws.drop_tab_onto_pane(a, shell, window, cx);
    });
    f.1.read_with(cx, |ws, _| {
        let runtime = ws.active_runtime();
        assert!(runtime.side.is_empty());
        assert_eq!(runtime.tabs.len(), 1, "merged, not left as a tab");
        assert_eq!(runtime.tabs[0].layout.pane_ids(), [shell, file_pane]);
        assert_eq!(runtime.focused_pane_id, file_pane);
        assert!(!ws.keyboard_is_aside());
        assert!(ws.main_area.pane_drop_hover.is_none());
    });
}
