use super::super::look::HomeLook;
use super::*;
use crate::remote_channel::transport::{PageRow, PageView};
use crate::surface::strings as s;

/// Every row of `view`, the groups read in order.
fn every_row<'a>(view: &'a PageView<'_>) -> Vec<&'a PageRow<'a>> {
    view.groups.iter().flat_map(|group| &group.rows).collect()
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
    let page = Board::new(vec![linked, unlinked, row("left out", None)], 2, None);
    let look = HomeLook::new(&page);
    let view = look.view();
    let rows = every_row(&view);
    assert_eq!(rows[0].emoji, Some("bell"));
    assert_eq!(rows[0].status, s::remote_channel::thread_status_needs_you());
    assert_eq!(
        rows[0].thread,
        Some((
            s::remote_channel::home_open_thread().as_str(),
            "https://x/p1"
        ))
    );
    assert_eq!(rows[0].subject, Some("Fix the bug"));
    assert_eq!(rows[1].subject, None, "no activity title");
    assert_eq!(rows[1].emoji, None);
    assert_eq!(rows[1].status, s::remote_channel::home_status_idle());
    assert!(rows[1].thread.is_none(), "no link known yet");
    assert_eq!(view.more, Some(s::remote_channel::home_more(1).as_str()));
    let full = HomeLook::new(&board(&["a"]));
    assert!(full.view().more.is_none(), "nothing left out");
}

/// Every row can be selected; only a working pane, or one waiting on the
/// person, can be stopped. The selected pane is marked and the last tap's
/// result heads the page.
#[test]
fn a_row_offers_stop_only_while_its_pane_runs() {
    use super::super::board::Outcome;
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
    let page = Board::new(rows, 10, Some(outcome));
    let look = HomeLook::new(&page);
    let view = look.view();
    let rows = every_row(&view);
    let stops: Vec<bool> = rows.iter().map(|row| row.stop.is_some()).collect();
    assert_eq!(stops, [true, true, false, false, false]);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.select.0, s::remote_channel::home_select());
        assert_eq!(
            row.select.1,
            page.token(index, super::super::token::Act::Select)
        );
    }
    assert_eq!(
        rows[0].stop.map(|(label, _)| label.to_owned()),
        Some(s::remote_channel::home_stop())
    );
    assert_eq!(
        rows[2].status,
        s::remote_channel::home_status_selected(s::remote_channel::thread_status_completed())
    );
    assert_eq!(
        rows[3].status,
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

/// Running panes, the ones waiting on the person first, are grouped apart
/// from the rest; the counts include the panes left out; an empty group is
/// left out; the help ends with a key built from the statuses.
#[test]
fn panes_are_grouped_by_whether_they_run_under_counts_and_help() {
    use crate::remote_channel::threads::status::Ending;
    let rows = vec![
        row("done", Some(ShownStatus::Ended(Ending::Completed))),
        row("working", Some(ShownStatus::Working { since: 1 })),
        row("asking", Some(ShownStatus::NeedsYou { since: 1 })),
        row("quiet", None),
        row("left out", None),
    ];
    let look = HomeLook::new(&Board::new(rows, 4, None));
    let view = look.view();
    let grouped: Vec<(&str, Vec<&str>)> = view
        .groups
        .iter()
        .map(|group| (group.title, group.rows.iter().map(|row| row.name).collect()))
        .collect();
    let running = s::remote_channel::home_group_running();
    let others = s::remote_channel::home_group_others();
    assert_eq!(
        grouped,
        [
            (running.as_str(), vec!["asking", "working"]),
            (others.as_str(), vec!["done", "quiet"]),
        ]
    );
    assert_eq!(
        view.summary,
        Some(s::remote_channel::home_summary(1, 1, 5).as_str())
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

    let idle = HomeLook::new(&board(&["a"]));
    let titles: Vec<&str> = idle.view().groups.iter().map(|group| group.title).collect();
    assert_eq!(titles, [others.as_str()], "no running heading without one");
    let empty = HomeLook::new(&Board::new(Vec::new(), 4, None));
    let view = empty.view();
    assert!(view.groups.is_empty() && view.summary.is_none());
}
