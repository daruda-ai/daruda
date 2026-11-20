use super::*;

fn dragged(from: Strip, index: usize, can_cross: bool) -> Dragged {
    Dragged {
        from,
        index,
        can_cross,
    }
}

#[test]
fn a_dragged_tab_lands_where_its_kind_allows() {
    use DropTarget::{End, PaneHalf, SideContent, Slot};
    use Strip::{Side, Tabs};
    use TabDropPlan::{Merge, MergeFromAside, Refuse, Reorder, SendAside, SendBack};

    let reorder = |strip, from, to| Reorder { strip, from, to };
    // (label, from, index, can cross, target, plan)
    type Row = (&'static str, Strip, usize, bool, DropTarget, TabDropPlan);
    let cases: [Row; 22] = [
        // Within a strip: the slot counts the dragged tab, so one past it
        // and its own are where it is.
        (
            "left of itself",
            Tabs,
            2,
            false,
            Slot(Tabs, 0),
            reorder(Tabs, 2, 0),
        ),
        (
            "right of itself",
            Tabs,
            0,
            false,
            Slot(Tabs, 3),
            reorder(Tabs, 0, 2),
        ),
        ("its own slot", Tabs, 1, true, Slot(Tabs, 1), Refuse),
        ("the slot after it", Tabs, 1, true, Slot(Tabs, 2), Refuse),
        ("its strip, no slot", Tabs, 1, true, End(Tabs), Refuse),
        (
            "a side tab reordered",
            Side,
            0,
            false,
            Slot(Side, 2),
            reorder(Side, 0, 1),
        ),
        ("a side tab in place", Side, 1, true, Slot(Side, 2), Refuse),
        ("its side strip, no slot", Side, 0, true, End(Side), Refuse),
        // A tab onto a pane of the tab on show merges, whatever it holds.
        (
            "a tab onto a pane",
            Tabs,
            1,
            false,
            PaneHalf,
            Merge { tab: 1 },
        ),
        (
            "a file tab onto a pane",
            Tabs,
            1,
            true,
            PaneHalf,
            Merge { tab: 1 },
        ),
        // Across: only a tab the other strip takes.
        (
            "a file tab to a side slot",
            Tabs,
            1,
            true,
            Slot(Side, 0),
            SendAside {
                tab: 1,
                at: Some(0),
            },
        ),
        (
            "a file tab to the side strip",
            Tabs,
            1,
            true,
            End(Side),
            SendAside { tab: 1, at: None },
        ),
        (
            "a file tab onto what is read aside",
            Tabs,
            1,
            true,
            SideContent,
            SendAside { tab: 1, at: None },
        ),
        (
            "a shell to a side slot",
            Tabs,
            0,
            false,
            Slot(Side, 0),
            Refuse,
        ),
        (
            "a shell to the side strip",
            Tabs,
            0,
            false,
            End(Side),
            Refuse,
        ),
        (
            "a shell onto what is read aside",
            Tabs,
            0,
            false,
            SideContent,
            Refuse,
        ),
        (
            "a file back to a slot",
            Side,
            0,
            true,
            Slot(Tabs, 2),
            SendBack {
                side: 0,
                at: Some(2),
            },
        ),
        (
            "a file back to the strip",
            Side,
            0,
            true,
            End(Tabs),
            SendBack { side: 0, at: None },
        ),
        (
            "a file back into a pane",
            Side,
            0,
            true,
            PaneHalf,
            MergeFromAside { side: 0 },
        ),
        (
            "a pull request to a slot",
            Side,
            0,
            false,
            Slot(Tabs, 0),
            Refuse,
        ),
        (
            "a pull request into a pane",
            Side,
            0,
            false,
            PaneHalf,
            Refuse,
        ),
        (
            "a side tab onto what it shows",
            Side,
            0,
            true,
            SideContent,
            Refuse,
        ),
    ];
    for (label, from, index, can_cross, target, plan) in cases {
        assert_eq!(
            plan_tab_drop(dragged(from, index, can_cross), target),
            plan,
            "{label}"
        );
    }
}
