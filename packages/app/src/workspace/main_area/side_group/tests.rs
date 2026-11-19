use super::*;

fn tab(id: u64) -> SideTab {
    SideTab {
        id,
        pane: id * 10,
        user_label: None,
    }
}

fn group(ids: &[u64]) -> SideGroup {
    let mut group = SideGroup::default();
    for &id in ids {
        group.push(tab(id));
    }
    group
}

fn ids(group: &SideGroup) -> Vec<u64> {
    group.tabs().iter().map(|tab| tab.id).collect()
}

#[test]
fn a_tab_added_is_the_one_shown_and_opens_a_collapsed_group() {
    let mut group = group(&[1, 2]);
    group.set_collapsed(true);
    assert_eq!(group.push(tab(3)), 2);
    assert_eq!(group.active_pane(), Some(30));
    assert!(!group.is_collapsed());
}

#[test]
fn removing_a_tab_keeps_the_right_one_shown() {
    // (label, tabs, shown, removed, left, shown after)
    type Row = (
        &'static str,
        &'static [u64],
        usize,
        usize,
        &'static [u64],
        Option<u64>,
    );
    let cases: [Row; 5] = [
        ("before the shown one", &[1, 2, 3], 2, 0, &[2, 3], Some(3)),
        ("the shown one", &[1, 2, 3], 1, 1, &[1, 3], Some(3)),
        ("the shown last one", &[1, 2, 3], 2, 2, &[1, 2], Some(2)),
        ("after the shown one", &[1, 2, 3], 0, 2, &[1, 2], Some(1)),
        ("the only one", &[1], 0, 0, &[], None),
    ];
    for (label, tabs, shown, removed, left, shown_after) in cases {
        let mut group = group(tabs);
        group.activate(shown);
        assert_eq!(
            group.remove(removed).map(|tab| tab.id),
            Some(tabs[removed]),
            "{label}"
        );
        assert_eq!(ids(&group), left, "{label}");
        assert_eq!(group.active_tab().map(|tab| tab.id), shown_after, "{label}");
    }
    assert!(group(&[1]).remove(4).is_none());
}

#[test]
fn an_emptied_group_gives_its_preview_slot_back() {
    let mut group = group(&[1]);
    group.preview_tab_id = Some(1);
    group.remove(0);
    assert_eq!(group.preview_tab_id, None);
    assert_eq!(group.shown_pane(), None);
}

#[test]
fn a_hidden_group_has_no_pane_on_show() {
    let mut group = group(&[1]);
    assert_eq!(group.shown_pane(), Some(10));
    group.set_collapsed(true);
    assert_eq!(group.shown_pane(), None);
    assert_eq!(group.active_pane(), Some(10), "its tab stays open");
}

#[test]
fn moving_a_tab_keeps_the_shown_one_shown() {
    let mut group = group(&[1, 2, 3]);
    group.activate(0);
    assert!(group.move_tab(0, 2));
    assert_eq!(ids(&group), [2, 3, 1]);
    assert_eq!(group.active_tab().map(|tab| tab.id), Some(1));
    assert!(group.move_tab(0, 1));
    assert_eq!(group.active_tab().map(|tab| tab.id), Some(1));
    assert!(!group.move_tab(1, 1));
    assert!(!group.move_tab(0, 3));
}

#[test]
fn a_ratio_stays_within_what_a_drag_may_reach() {
    let mut group = SideGroup::default();
    for (asked, kept) in [(0.5, 0.5), (0.0, MIN_RATIO), (2.0, MAX_RATIO)] {
        group.set_ratio(asked);
        assert_eq!(group.ratio(), kept, "{asked}");
    }
    group.set_ratio(f32::NAN);
    assert_eq!(group.ratio(), MAX_RATIO);
}

#[test]
fn both_groups_are_drawn_only_where_each_keeps_its_least_width() {
    const MIN: f32 = 300.0;
    let (tabs, side) = (KeyboardIn::Tabs, KeyboardIn::Side);
    let both = |tabs, side| GroupsShown::Both { tabs, side };
    let mut group = group(&[1]);
    // (label, share, width, keyboard, drawn)
    let cases = [
        ("an even share", 0.5, 1001.0, tabs, both(500.0, 500.0)),
        (
            "a share under the least width",
            MIN_RATIO,
            1001.0,
            tabs,
            both(700.0, 300.0),
        ),
        (
            "a share over what leaves it",
            MAX_RATIO,
            1001.0,
            tabs,
            both(300.0, 700.0),
        ),
        (
            "no room, keyboard on the left",
            0.5,
            500.0,
            tabs,
            GroupsShown::Tabs,
        ),
        (
            "no room, keyboard aside",
            0.5,
            500.0,
            side,
            GroupsShown::Side,
        ),
    ];
    for (label, share, width, keyboard, drawn) in cases {
        group.set_ratio(share);
        assert_eq!(group.shown_in(width, MIN, keyboard), drawn, "{label}");
    }
    // Collapsed or empty: the tabs alone, whatever the width.
    group.set_collapsed(true);
    assert_eq!(group.shown_in(2000.0, MIN, side), GroupsShown::Tabs);
    assert_eq!(
        SideGroup::default().shown_in(2000.0, MIN, side),
        GroupsShown::Tabs
    );
}

#[test]
fn a_shown_set_says_what_width_the_tabs_get() {
    let both = GroupsShown::Both {
        tabs: 600.0,
        side: 400.0,
    };
    assert_eq!(both.tabs_width(1001.0), 600.0);
    assert_eq!(GroupsShown::Tabs.tabs_width(1001.0), 1001.0);
    assert!(both.draws_tabs() && GroupsShown::Tabs.draws_tabs());
    assert!(!GroupsShown::Side.draws_tabs());
}

#[test]
fn a_divider_moved_left_widens_the_side_group() {
    // 1001 wide, so the two share 1000 beside the divider.
    assert_eq!(SideGroup::ratio_after_drag(0.4, -100.0, 1001.0), Some(0.5));
    assert_eq!(SideGroup::ratio_after_drag(0.4, 100.0, 1001.0), Some(0.3));
    assert_eq!(SideGroup::ratio_after_drag(0.4, 10.0, 1.0), None);
}
