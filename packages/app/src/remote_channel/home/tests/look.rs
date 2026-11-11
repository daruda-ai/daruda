use super::super::look::HomeLook;
use super::*;
use crate::remote_channel::transport::{PagePart, PageRow, PageView};
use crate::surface::strings as s;

/// Every row of `view`, top to bottom: a pane in the strip and in its
/// project twice.
fn every_row<'a>(view: &'a PageView<'_>) -> Vec<&'a PageRow<'a>> {
    view.parts
        .iter()
        .flat_map(|part| match part {
            PagePart::Rows(rows) => rows.as_slice(),
            _ => &[],
        })
        .collect()
}

/// The listing in short: `#` a heading, `-` a label, `[..]` rows by name.
fn outline(view: &PageView<'_>) -> Vec<String> {
    view.parts
        .iter()
        .map(|part| match part {
            PagePart::Heading(text) => format!("#{text}"),
            PagePart::Label(text) => format!("-{text}"),
            PagePart::Rows(rows) => {
                let names: Vec<&str> = rows.iter().map(|row| row.name).collect();
                format!("[{}]", names.join(","))
            }
        })
        .collect()
}

/// A pane with no status since daruda started reads idle, without an emoji;
/// a known status shows its emoji; only a known link gets a button.
#[test]
fn each_row_reads_its_status_and_links_only_a_known_thread() {
    let mut linked = row("asking", Some(ShownStatus::NeedsYou { since: 1 }));
    linked.thread = Some("1.1".into());
    linked.link = Some("https://x/p1".into());
    linked.subject = Some("Fix the bug".into());
    let mut unlinked = row("quiet", None);
    unlinked.thread = Some("2.2".into());
    let page = flat(vec![linked, unlinked]);
    let look = HomeLook::new(&page);
    let view = look.view();
    let rows = every_row(&view);
    assert_eq!(
        rows.len(),
        3,
        "the asking pane in the strip and its project"
    );
    let (asking, quiet) = (rows[1], rows[2]);
    assert_eq!(asking.emoji, Some("bell"));
    assert_eq!(asking.status, s::remote_channel::thread_status_needs_you());
    assert_eq!(
        asking.thread,
        Some((
            s::remote_channel::home_open_thread().as_str(),
            "https://x/p1"
        ))
    );
    assert_eq!(asking.subject, Some("Fix the bug"));
    assert_eq!(quiet.subject, None, "no activity title");
    assert_eq!(quiet.emoji, None);
    assert_eq!(quiet.status, s::remote_channel::home_status_idle());
    assert!(quiet.thread.is_none(), "no link known yet");
    assert!(view.more.is_none(), "nothing left out");
}

/// Every row can be selected; only a working pane, or one waiting on the
/// person, can be stopped. A pane shown twice carries the same values both
/// times. The selected pane is marked and the last tap's result heads the
/// page.
#[test]
fn a_row_offers_stop_only_while_its_pane_runs() {
    use super::super::board::Outcome;
    use super::super::token::Act;
    use crate::remote_channel::threads::status::Ending;
    let mut picked = row("done", Some(ShownStatus::Ended(Ending::Completed)));
    picked.selected = true;
    let rows = vec![
        row("asking", Some(ShownStatus::NeedsYou { since: 1 })),
        row("working", Some(ShownStatus::Working { since: 1 })),
        picked,
        row("quiet", None),
        row("closed", Some(ShownStatus::Closed)),
    ];
    let outcome = Outcome {
        at: 0,
        text: "Stopped".into(),
    };
    let page = Board::new(
        vec![window(
            1,
            vec![alone(project("p", vec![lane("main", rows)]))],
        )],
        ROOMY,
        Some(outcome),
    );
    let look = HomeLook::new(&page);
    let view = look.view();
    let rows = every_row(&view);
    let stops: Vec<bool> = rows.iter().map(|row| row.stop.is_some()).collect();
    assert_eq!(stops, [true, true, true, false, false, false]);
    assert_eq!(rows[0].select, rows[1].select, "the strip's row is its own");
    assert_eq!(rows[0].stop, rows[1].stop);
    for (index, row) in rows[1..].iter().enumerate() {
        assert_eq!(row.select.0, s::remote_channel::home_select());
        assert_eq!(row.select.1, page.token(index, Act::Select));
    }
    assert_eq!(
        rows[0].stop.map(|(label, _)| label.to_owned()),
        Some(s::remote_channel::home_stop())
    );
    assert_eq!(
        rows[3].status,
        s::remote_channel::home_status_selected(s::remote_channel::thread_status_completed())
    );
    assert_eq!(
        rows[4].status,
        s::remote_channel::home_status_idle(),
        "unselected, unmarked"
    );
    assert_eq!(
        view.outcome,
        Some(s::remote_channel::home_result(chrono::DateTime::UNIX_EPOCH, "Stopped").as_str())
    );
    assert!(
        HomeLook::new(&board(&["a"])).view().outcome.is_none(),
        "no tap yet"
    );
}

/// The strip, windows, groups, projects and lanes read in the person's
/// language; the counts include the panes left out, and the help ends with
/// a key built from the statuses.
#[test]
fn headings_and_labels_name_what_follows_under_counts_and_help() {
    let asking = || row("asking", Some(ShownStatus::NeedsYou { since: 1 }));
    let windows = vec![
        window(
            1,
            vec![group(
                "tools",
                vec![
                    project("cli", vec![lane("main", vec![row("quiet", None)])]),
                    project(
                        "web",
                        vec![
                            lane("main", vec![row("w", None)]),
                            lane("fix", vec![asking()]),
                        ],
                    ),
                ],
            )],
        ),
        window(
            2,
            vec![alone(project(
                "solo",
                vec![lane("main", vec![row("left out", None)])],
            ))],
        ),
    ];
    let budget = PageBudget {
        blocks: 11,
        ..ROOMY
    };
    let look = HomeLook::new(&Board::new(windows, budget, None));
    let view = look.view();
    let needs_you = format!("#{}", s::remote_channel::home_needs_you());
    let window_1 = format!("#{}", s::remote_channel::home_window(1));
    assert_eq!(
        outline(&view),
        [
            needs_you.as_str(),
            "[asking]",
            window_1.as_str(),
            "#tools",
            "-cli",
            "[quiet]",
            &format!("-{}", s::control::lane_path("web", "main")),
            "[w]",
            &format!("-{}", s::control::lane_path("web", "fix")),
            "[asking]",
        ]
    );
    assert_eq!(view.more, Some(s::remote_channel::home_more(1).as_str()));
    assert_eq!(
        view.summary,
        Some(s::remote_channel::home_summary(0, 1, 4).as_str())
    );
    let help = &view.help;
    assert_eq!(help.title, s::remote_channel::home_help_title());
    assert_eq!(help.lines.len(), 3);
    assert!(help.lines[1].contains(&s::remote_channel::home_select()));
    let legend: Vec<(&str, String)> = help
        .legend
        .iter()
        .map(|(emoji, label)| (*emoji, (*label).to_owned()))
        .collect();
    assert_eq!(legend, ShownStatus::legend());
    assert_eq!(legend.len(), 7, "every kind of status once");
    assert_eq!(
        legend[0],
        (
            ShownStatus::Working { since: 0 }.view().0,
            s::remote_channel::home_legend_working()
        ),
        "working without its time"
    );

    let empty = HomeLook::new(&flat(Vec::new()));
    let view = empty.view();
    assert!(view.parts.is_empty() && view.summary.is_none());
}
