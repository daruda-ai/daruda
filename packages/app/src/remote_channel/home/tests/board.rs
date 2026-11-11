use super::*;
use crate::remote_channel::threads::status::Ending;

fn names(board: &Board) -> Vec<&str> {
    board.rows.iter().map(|row| row.name.as_str()).collect()
}

/// What the person must act on leads, then what is running; everything else
/// keeps the order `/list` gives it.
#[test]
fn panes_needing_the_person_lead_then_working_ones() {
    let rows = vec![
        row("done", Some(ShownStatus::Ended(Ending::Completed))),
        row("working", Some(ShownStatus::Working { since: 1 })),
        row("unknown", None),
        row("asking", Some(ShownStatus::NeedsYou { since: 2 })),
        row("closed", Some(ShownStatus::Closed)),
        row("also working", Some(ShownStatus::Working { since: 3 })),
    ];
    let board = Board::new(rows, 10);
    assert_eq!(
        names(&board),
        [
            "asking",
            "working",
            "also working",
            "done",
            "unknown",
            "closed"
        ]
    );
    assert_eq!(board.omitted, 0);
}

#[test]
fn a_board_keeps_what_fits_and_counts_the_rest() {
    let cases = [
        (5, 3, 3, 2, "more panes than room"),
        (5, 5, 5, 0, "exactly full"),
        (0, 5, 0, 0, "no panes"),
    ];
    for (panes, capacity, shown, omitted, label) in cases {
        let rows = (0..panes).map(|i| row(&i.to_string(), None)).collect();
        let board = Board::new(rows, capacity);
        let first: Vec<String> = (0..shown).map(|i| i.to_string()).collect();
        assert_eq!(names(&board), first, "{label}: the first in order");
        assert_eq!(board.omitted, omitted, "{label}");
    }
}

/// A row links its thread once the link is known; the roots still missing
/// one are the ones to ask for.
#[test]
fn rows_take_the_links_known_for_their_threads() {
    let mut threaded = row("a", None);
    threaded.thread = Some("1.1".into());
    let mut unknown = row("b", None);
    unknown.thread = Some("2.2".into());
    let mut board = Board::new(vec![threaded, unknown, row("c", None)], 10);
    assert_eq!(board.unlinked().collect::<Vec<_>>(), ["1.1", "2.2"]);
    board.link(|root| (root == "1.1").then(|| "https://x/p1".to_owned()));
    assert_eq!(board.rows[0].link.as_deref(), Some("https://x/p1"));
    assert_eq!(board.rows[1].link, None);
    assert_eq!(board.rows[2].link, None, "a pane without a thread");
    assert_eq!(board.unlinked().collect::<Vec<_>>(), ["2.2"]);
}

/// A tap names the row of the board it was shown on: another generation, or
/// a row past the end, names nothing.
#[test]
fn a_tap_resolves_only_against_its_own_board() {
    use super::super::token::{Act, Numbering, Tap};
    let mut board = Board::new(vec![row("a", None), row("b", None)], 10);
    let mut numbering = Numbering::default();
    board.number(&mut numbering);
    let generation = board.generation;
    let tap = |generation, index| Tap {
        generation,
        index,
        act: Act::Stop,
    };
    let cases = [
        (tap(generation, 1), Some(board.rows[1].pane), "its own row"),
        (tap(generation.wrapping_sub(1), 1), None, "an older board"),
        (tap(generation, 2), None, "past the last row"),
    ];
    for (tap, want, label) in cases {
        assert_eq!(board.resolve(tap), want, "{label}");
    }
    assert_eq!(
        Tap::parse(&board.token(1, Act::Select)),
        Some(Tap {
            act: Act::Select,
            ..tap(generation, 1)
        })
    );
}
