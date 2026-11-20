use super::*;
use crate::workspace::main_area::pane_tree::{PaneId, PaneLayout};

fn tab(id: u64) -> TabEntry {
    TabEntry {
        id: TabId(id),
        layout: PaneLayout::Pane(PaneId(id * 10)),
        last_focused_pane: PaneId(id * 10),
        user_label: None,
    }
}

/// A strip of `ids`, each opened in turn, so the last is the one shown.
fn strip(ids: &[u64]) -> TabStrip {
    let mut strip = TabStrip::default();
    for &id in ids {
        strip.open(tab(id));
    }
    strip
}

fn ids(strip: &TabStrip) -> Vec<u64> {
    strip.iter().map(|tab| tab.id.0).collect()
}

fn shown(strip: &TabStrip) -> Option<u64> {
    strip.active_tab().map(|tab| tab.id.0)
}

fn recent(strip: &TabStrip) -> Vec<u64> {
    strip.recent().iter().map(|id| id.0).collect()
}

/// Show the tabs with these ids in turn, as the user would.
fn visit(strip: &mut TabStrip, order: &[u64]) {
    for &id in order {
        let index = strip.position_of_id(TabId(id)).expect("a tab of the strip");
        strip.activate(index, TabHistory::Record);
    }
}

#[test]
fn adding_shows_only_when_asked() {
    let mut strip = TabStrip::default();
    strip.push(tab(1));
    strip.push(tab(2));
    assert_eq!(shown(&strip), Some(1), "the first tab is the one shown");
    assert!(recent(&strip).is_empty(), "nothing was left to get here");

    assert_eq!(strip.open(tab(3)), 2);
    assert_eq!(shown(&strip), Some(3));
    assert_eq!(recent(&strip), [1]);

    let mut empty = TabStrip::default();
    empty.open(tab(1));
    assert!(recent(&empty).is_empty(), "the first tab left none behind");
}

#[test]
fn each_tab_shown_before_is_remembered_once() {
    let mut strip = strip(&[1, 2, 3]);
    visit(&mut strip, &[2, 3, 2, 1]);
    assert_eq!(
        recent(&strip),
        [3, 2],
        "most recent last, the shown one out"
    );

    let before = recent(&strip);
    assert!(!strip.activate(0, TabHistory::Record), "already shown");
    assert!(!strip.activate(9, TabHistory::Record), "no such tab");
    assert_eq!(recent(&strip), before);
}

#[test]
fn a_look_ahead_leaves_no_trail() {
    let mut strip = strip(&[1, 2, 3]);
    let before = recent(&strip);
    assert!(strip.activate(0, TabHistory::Skip));
    assert_eq!(shown(&strip), Some(1));
    assert_eq!(recent(&strip), [2], "the tab looked at is no longer behind");
    assert!(strip.activate(2, TabHistory::Skip));
    assert_eq!(shown(&strip), Some(3));
    assert_eq!(recent(&strip), [2]);
    assert_eq!(before, [1, 2]);
}

#[test]
fn removing_a_tab_settles_which_is_shown() {
    // (label, visits after opening 1..=4, removed index, left, shown after)
    type Row = (&'static str, &'static [u64], usize, &'static [u64], u64);
    let cases: [Row; 6] = [
        ("the shown one goes back", &[2], 1, &[1, 3, 4], 4),
        ("back past a closed tab", &[1, 3], 2, &[1, 2, 4], 1),
        ("before the shown one", &[3], 0, &[2, 3, 4], 3),
        ("after the shown one", &[2], 3, &[1, 2, 3], 2),
        ("the shown last one", &[], 3, &[1, 2, 3], 3),
        ("one never left", &[1], 3, &[1, 2, 3], 1),
    ];
    for (label, visits, removed, left, shown_after) in cases {
        let mut strip = strip(&[1, 2, 3, 4]);
        visit(&mut strip, visits);
        let gone = strip.remove(removed).map(|tab| tab.id.0);
        assert!(gone.is_some(), "{label}");
        assert_eq!(ids(&strip), left, "{label}");
        assert_eq!(shown(&strip), Some(shown_after), "{label}");
        assert!(
            !recent(&strip).contains(&gone.unwrap()),
            "{label}: a closed tab is not gone back to"
        );
        assert!(
            !recent(&strip).contains(&shown_after),
            "{label}: the shown tab is not behind itself"
        );
    }
}

#[test]
fn with_nothing_behind_the_neighbour_is_shown() {
    let mut strip = TabStrip::restored(vec![tab(1), tab(2), tab(3)], 1);
    assert_eq!(strip.remove(1).map(|tab| tab.id.0), Some(2));
    assert_eq!(shown(&strip), Some(3), "the tab that took its slot");
    assert_eq!(strip.remove(1).map(|tab| tab.id.0), Some(3));
    assert_eq!(shown(&strip), Some(1), "else the one before");
    assert_eq!(strip.remove(0).map(|tab| tab.id.0), Some(1));
    assert_eq!(shown(&strip), None);
    assert_eq!(strip.active_index(), 0);
    assert!(strip.remove(0).is_none());
}

#[test]
fn moving_and_inserting_keep_the_shown_tab_shown() {
    // (label, shown id, from, to, order after)
    type Row = (&'static str, u64, usize, usize, &'static [u64]);
    let cases: [Row; 4] = [
        ("the shown one right", 1, 0, 2, &[2, 3, 1]),
        ("the shown one left", 3, 2, 0, &[3, 1, 2]),
        ("across the shown one", 2, 0, 2, &[2, 3, 1]),
        ("clear of the shown one", 1, 1, 2, &[1, 3, 2]),
    ];
    for (label, shown_id, from, to, after) in cases {
        let mut strip = strip(&[1, 2, 3]);
        visit(&mut strip, &[shown_id]);
        let behind = recent(&strip);
        assert!(strip.move_tab(from, to), "{label}");
        assert_eq!(ids(&strip), after, "{label}");
        assert_eq!(shown(&strip), Some(shown_id), "{label}");
        assert_eq!(recent(&strip), behind, "{label}: a move is not a visit");
    }
    let mut strip = strip(&[1, 2, 3]);
    assert!(!strip.move_tab(1, 1));
    assert!(!strip.move_tab(0, 3));

    visit(&mut strip, &[2]);
    assert_eq!(strip.insert(0, tab(4)), 0);
    assert_eq!(shown(&strip), Some(2), "inserted before the shown one");
    assert_eq!(strip.insert(1, tab(5)), 1);
    assert_eq!(shown(&strip), Some(2), "inserted in its slot");
    assert_eq!(strip.insert(99, tab(6)), 5);
    assert_eq!(ids(&strip), [4, 5, 1, 2, 3, 6]);
    assert_eq!(shown(&strip), Some(2), "inserted after it");

    let mut empty = TabStrip::default();
    empty.insert(0, tab(1));
    assert_eq!(shown(&empty), Some(1));
}

#[test]
fn a_restored_strip_shows_a_tab_it_has() {
    let strip = TabStrip::restored(vec![tab(1), tab(2)], 99);
    assert_eq!(shown(&strip), Some(2));
    assert_eq!(TabStrip::restored(Vec::new(), 3).active_index(), 0);
}
