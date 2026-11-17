use daruda_content::diff::{build_diff_rows, parse_diff_hunks};

use super::*;

fn rows(patch: &str) -> Vec<VisualRow> {
    build_diff_rows(&parse_diff_hunks(patch), false, "\\ No newline")
}

const PATCH: &str = "@@ -1,4 +1,5 @@\n keep\n-old a\n-old b\n+new a\n+new b\n+new c\n tail";

#[test]
fn removed_runs_sit_beside_the_added_runs_that_follow_them() {
    let rows = rows(PATCH);
    let split = split_rows(&rows);
    let cell = |c: &Option<Cell>| c.as_ref().map(|c| rows[c.row].content.clone());
    let shape: Vec<(Option<String>, Option<String>)> = split
        .iter()
        .filter_map(|row| match row {
            SplitRow::Pair { old, new } => Some((cell(old), cell(new))),
            SplitRow::Banner(_) => None,
        })
        .collect();
    let s = |t: &str| Some(t.to_owned());
    assert_eq!(
        shape,
        [
            (s("keep"), s("keep")),
            (s("old a"), s("new a")),
            (s("old b"), s("new b")),
            (None, s("new c")),
            (s("tail"), s("tail")),
        ]
    );
    assert!(
        matches!(split[0], SplitRow::Banner(0)),
        "the hunk header spans both"
    );
}

#[test]
fn a_row_holds_its_line_on_each_side_it_belongs_to() {
    let rows = rows(PATCH);
    let on = |side| {
        rows.iter()
            .map(|row| line_on(row, side))
            .collect::<Vec<_>>()
    };
    // Header, keep, old a, old b, new a, new b, new c, tail.
    assert_eq!(
        on(Side::Old),
        [None, Some(1), Some(2), Some(3), None, None, None, Some(4)],
        "unchanged lines count on the old side too"
    );
    assert_eq!(
        on(Side::New),
        [
            None,
            Some(1),
            None,
            None,
            Some(2),
            Some(3),
            Some(4),
            Some(5)
        ]
    );
}

#[test]
fn an_old_side_range_holds_and_copies_the_unchanged_lines_inside_it() {
    let rows = rows(PATCH);
    let range = LineRange {
        path: "a.rs".into(),
        side: Side::Old,
        start: 1,
        end: 3,
        revision: "abc1234".into(),
    };
    assert_eq!(selected_text(&rows, &range), "keep\nold a\nold b");
    let new = LineRange {
        side: Side::New,
        start: 4,
        end: 5,
        ..range
    };
    assert_eq!(selected_text(&rows, &new), "new c\ntail");
}

#[test]
fn a_selection_stretches_only_within_its_file_side_and_revision() {
    let one = select(None, "a.rs", Side::New, 7, "abc1234", false);
    let stretched = select(Some(&one), "a.rs", Side::New, 3, "abc1234", true);
    assert_eq!((stretched.start, stretched.end), (3, 7));
    // (label, path, side, revision, extend, start..=end)
    let cases = [
        (
            "a plain click starts over",
            "a.rs",
            Side::New,
            "abc1234",
            false,
            (9, 9),
        ),
        (
            "another side starts over",
            "a.rs",
            Side::Old,
            "abc1234",
            true,
            (9, 9),
        ),
        (
            "another file starts over",
            "b.rs",
            Side::New,
            "abc1234",
            true,
            (9, 9),
        ),
        (
            "another revision starts over",
            "a.rs",
            Side::New,
            "def5678",
            true,
            (9, 9),
        ),
    ];
    for (label, path, side, revision, extend, range) in cases {
        let next = select(Some(&stretched), path, side, 9, revision, extend);
        assert_eq!((next.start, next.end), range, "{label}");
    }
    let rows = rows(PATCH);
    let old = select(None, "a.rs", Side::Old, 2, "abc1234", false);
    let held: Vec<bool> = rows.iter().map(|row| old.holds(row)).collect();
    assert_eq!(
        held,
        [false, false, true, false, false, false, false, false]
    );
    assert_eq!(stretched.reference(), "a.rs:3-7 (new) @ abc1234");
    assert_eq!(old.reference(), "a.rs:2 (old) @ abc1234");
}
