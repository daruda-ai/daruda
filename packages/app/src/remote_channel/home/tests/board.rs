use super::super::board::Part;
use super::*;
use crate::remote_channel::threads::status::Ending;

fn names(board: &Board) -> Vec<&str> {
    board.rows().iter().map(|row| row.name.as_str()).collect()
}

/// The listing in short: `#` a heading, `-` a label, `[..]` a run of rows by
/// name, `!` the strip's heading.
fn outline(board: &Board) -> Vec<String> {
    board
        .parts()
        .into_iter()
        .map(|part| match part {
            Part::NeedsYou => "!needs you".to_owned(),
            Part::Window(number) => format!("#window {number}"),
            Part::Group(name) | Part::Project(name) => format!("#{name}"),
            Part::GroupedProject(name) => format!("-{name}"),
            Part::Lane { project, lane } => format!("-{project}/{lane}"),
            Part::Rows(rows) => {
                let names: Vec<&str> = rows
                    .iter()
                    .map(|&index| board.rows()[index].name.as_str())
                    .collect();
                format!("[{}]", names.join(","))
            }
        })
        .collect()
}

fn asking(name: &str) -> PaneRow {
    row(name, Some(ShownStatus::NeedsYou { since: 1 }))
}

fn working(name: &str) -> PaneRow {
    row(name, Some(ShownStatus::Working { since: 1 }))
}

/// Two windows, each named; a group's projects under its heading, one named
/// by a label and one, with panes in two lanes, by its lanes'; an ungrouped
/// project under its own heading, its lanes labelled too. Every pane keeps
/// the arrangement's order whatever its status.
fn sidebar() -> Vec<WindowPanes> {
    vec![
        window(
            1,
            vec![
                group(
                    "tools",
                    vec![
                        project(
                            "cli",
                            vec![lane("main", vec![row("c1", None), working("c2")])],
                        ),
                        project(
                            "web",
                            vec![
                                lane("main", vec![row("w1", None)]),
                                lane("fix", vec![asking("w2")]),
                            ],
                        ),
                    ],
                ),
                alone(project(
                    "app",
                    vec![
                        lane("main", vec![row("a1", None)]),
                        lane("ui", vec![row("a2", None)]),
                    ],
                )),
            ],
        ),
        window(2, Vec::new()),
        window(
            3,
            vec![alone(project(
                "solo",
                vec![lane("main", vec![row("s1", None)])],
            ))],
        ),
    ]
}

#[test]
fn panes_are_laid_out_as_the_sidebar_arranges_them() {
    let board = Board::new(sidebar(), ROOMY, None);
    assert_eq!(
        outline(&board),
        [
            "!needs you",
            "[w2]",
            "#window 1",
            "#tools",
            "-cli",
            "[c1,c2]",
            "-web/main",
            "[w1]",
            "-web/fix",
            "[w2]",
            "#app",
            "-app/main",
            "[a1]",
            "-app/ui",
            "[a2]",
            "#window 3",
            "#solo",
            "[s1]",
        ]
    );
    assert_eq!(
        names(&board),
        ["c1", "c2", "w1", "w2", "a1", "a2", "s1"],
        "every pane once, in the arrangement's order"
    );
    assert_eq!(board.omitted(), 0);
    let counts = board.counts();
    assert_eq!((counts.working, counts.needs_you, counts.total), (1, 1, 7));
}

/// The strip shows a pane that also shows in its project with the same row,
/// so both carry the same button values.
#[test]
fn a_pane_in_the_strip_is_the_same_row_in_its_project() {
    let board = Board::new(sidebar(), ROOMY, None);
    let runs: Vec<&[usize]> = board
        .parts()
        .into_iter()
        .filter_map(|part| match part {
            Part::Rows(rows) => Some(rows),
            _ => None,
        })
        .collect();
    let strip = runs[0][0];
    assert!(runs[1..].iter().any(|rows| rows.contains(&strip)));
    assert_eq!(board.rows()[strip].name, "w2");
}

/// One window with panes is not named, however many windows there are; no
/// pane needing the person, no strip; a project with one lane is not given
/// lane labels.
#[test]
fn a_lone_window_and_a_quiet_page_take_no_extra_headings() {
    let board = Board::new(
        vec![
            window(1, Vec::new()),
            window(
                2,
                vec![group(
                    "g",
                    vec![project("p", vec![lane("main", vec![working("a")])])],
                )],
            ),
        ],
        ROOMY,
        None,
    );
    assert_eq!(outline(&board), ["#g", "-p", "[a]"]);
    assert_eq!(outline(&flat(Vec::new())), Vec::<String>::new());
    let ended = flat(vec![row(
        "done",
        Some(ShownStatus::Ended(Ending::Completed)),
    )]);
    assert_eq!(outline(&ended), ["#p", "[done]"]);
}

/// Projects that do not fit are left out whole, from the first that does
/// not fit on, with the panes shown nowhere counted; a pane needing the
/// person still shows in the strip. The listing stays within the budget.
#[test]
fn a_board_keeps_whole_projects_that_fit_and_counts_the_rest() {
    let budget = |blocks, cards| PageBudget {
        blocks,
        cards_per_block: 2,
        cards,
    };
    // Room for `sidebar()`: the strip 2 blocks; window 1 + tools + cli + its
    // carousel 4; web's two labels and carousels 4; app's heading, two
    // labels and carousels 5; window 3 + solo + carousel 3. Cards: 1 + 7.
    let cases = [
        (
            budget(18, 100),
            8,
            "everything, exactly",
            vec!["c1", "c2", "w1", "w2", "a1", "a2", "s1"],
            0,
        ),
        (
            budget(17, 100),
            8,
            "solo does not fit",
            vec!["c1", "c2", "w1", "w2", "a1", "a2"],
            1,
        ),
        (
            budget(12, 100),
            8,
            "app and all after",
            vec!["c1", "c2", "w1", "w2"],
            3,
        ),
        (
            budget(9, 100),
            8,
            "web would be cut in part, so whole",
            vec!["c1", "c2", "w2"],
            4,
        ),
        (
            budget(5, 100),
            8,
            "no project fits; the strip still shows",
            vec!["w2"],
            6,
        ),
        (
            budget(18, 4),
            4,
            "cards run out at web",
            vec!["c1", "c2", "w2"],
            4,
        ),
    ];
    for (budget, cards, label, shown, omitted) in cases {
        let board = Board::new(sidebar(), budget, None);
        assert_eq!(names(&board), shown, "{label}");
        assert_eq!(board.omitted(), omitted, "{label}");
        assert_eq!(
            board.counts().total,
            7,
            "{label}: counts include the left out"
        );
        let (blocks, taken) = board.room_taken(budget);
        assert!(blocks <= budget.blocks, "{label}: {blocks} blocks");
        assert!(taken <= cards, "{label}: {taken} cards");
    }
    let cut = Board::new(sidebar(), budget(12, 100), None);
    assert_eq!(
        outline(&cut),
        [
            "!needs you",
            "[w2]",
            "#window 1",
            "#tools",
            "-cli",
            "[c1,c2]",
            "-web/main",
            "[w1]",
            "-web/fix",
            "[w2]",
        ],
        "the group keeps the projects that fit"
    );
}

/// A strip with no room for all its panes keeps the first that fit.
#[test]
fn a_strip_too_long_is_cut_to_the_room() {
    let rows = (0..7).map(|i| asking(&format!("n{i}"))).collect();
    let budget = PageBudget {
        blocks: 3,
        cards_per_block: 2,
        cards: 100,
    };
    let board = Board::new(
        vec![window(
            1,
            vec![alone(project("p", vec![lane("main", rows)]))],
        )],
        budget,
        None,
    );
    assert_eq!(outline(&board), ["!needs you", "[n0,n1,n2,n3]"]);
    assert_eq!(board.omitted(), 3);
}

/// The buttons keep their generation while the same panes show in the same
/// order, even as one comes to need the person; another pane takes a new one.
#[test]
fn only_other_panes_renumber_the_buttons() {
    let mut numbering = super::super::token::Numbering::default();
    let mut numbered = |rows: Vec<PaneRow>| {
        let mut board = flat(rows);
        board.number(&mut numbering);
        board.generation()
    };
    let quiet = numbered(vec![row("a", None), row("b", None)]);
    assert_eq!(numbered(vec![row("a", None), asking("b")]), quiet);
    assert_ne!(numbered(vec![row("a", None), row("c", None)]), quiet);
}

/// A row links its thread once the link is known; the roots still missing
/// one are the ones to ask for.
#[test]
fn rows_take_the_links_known_for_their_threads() {
    let mut threaded = row("a", None);
    threaded.thread = Some("1.1".into());
    let mut unknown = row("b", None);
    unknown.thread = Some("2.2".into());
    let mut board = flat(vec![threaded, unknown, row("c", None)]);
    assert_eq!(board.unlinked().collect::<Vec<_>>(), ["1.1", "2.2"]);
    board.link(|root| (root == "1.1").then(|| "https://x/p1".to_owned()));
    assert_eq!(board.rows()[0].link.as_deref(), Some("https://x/p1"));
    assert_eq!(board.rows()[1].link, None);
    assert_eq!(board.rows()[2].link, None, "a pane without a thread");
    assert_eq!(board.unlinked().collect::<Vec<_>>(), ["2.2"]);
}

/// A tap names the row of the board it was shown on: another generation, or
/// a row past the end, names nothing.
#[test]
fn a_tap_resolves_only_against_its_own_board() {
    use super::super::token::{Act, Numbering, Tap};
    let mut board = flat(vec![row("a", None), row("b", None)]);
    let mut numbering = Numbering::default();
    board.number(&mut numbering);
    let generation = board.generation();
    let tap = |generation, index| Tap {
        generation,
        index,
        act: Act::Stop,
    };
    let cases = [
        (
            tap(generation, 1),
            Some(board.rows()[1].pane),
            "its own row",
        ),
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
