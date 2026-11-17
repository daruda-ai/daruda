use daruda_github::RepoId;
use gpui::{Bounds, point, px, size};

use super::*;

fn keys(numbers: &[u64]) -> Vec<PrKey> {
    let repo = RepoId::new("github.com", "acme", "app");
    numbers.iter().map(|n| (repo.clone(), *n)).collect()
}

#[test]
fn a_step_moves_within_the_shown_rows_and_stops_at_either_end() {
    let order = keys(&[1, 2, 3]);
    let other = keys(&[9]).remove(0);
    // (label, cursor, delta, expected)
    let cases: [(&str, Option<&PrKey>, isize, Option<u64>); 7] = [
        ("first down selects the first row", None, 1, Some(1)),
        ("first up selects the last row", None, -1, Some(3)),
        ("down moves one row", Some(&order[0]), 1, Some(2)),
        ("up moves one row", Some(&order[1]), -1, Some(1)),
        ("down stops at the last row", Some(&order[2]), 1, Some(3)),
        ("up stops at the first row", Some(&order[0]), -1, Some(1)),
        (
            "a row the list no longer shows restarts",
            Some(&other),
            1,
            Some(1),
        ),
    ];
    for (label, cursor, delta, expected) in cases {
        assert_eq!(
            step(&order, cursor, delta).map(|(_, n)| n),
            expected,
            "{label}"
        );
    }
    assert_eq!(step(&[], None, 1), None, "an empty list selects nothing");
}

#[test]
fn reveal_scrolls_only_as_far_as_the_row_needs() {
    let viewport = Bounds::new(point(px(0.), px(100.)), size(px(400.), px(200.)));
    let row =
        |top: f32, height: f32| Bounds::new(point(px(0.), px(top)), size(px(400.), px(height)));
    let offset = px(-50.);
    // (label, row, expected offset)
    let cases = [
        ("a fully visible row stays put", row(150., 30.), None),
        (
            "a row above scrolls down to it",
            row(80., 30.),
            Some(px(-30.)),
        ),
        (
            "a row below scrolls up to it",
            row(290., 30.),
            Some(px(-70.)),
        ),
        (
            "a row taller than the view aligns its top",
            row(120., 260.),
            Some(px(-70.)),
        ),
    ];
    for (label, row, expected) in cases {
        assert_eq!(reveal(viewport, row, offset), expected, "{label}");
    }
}
