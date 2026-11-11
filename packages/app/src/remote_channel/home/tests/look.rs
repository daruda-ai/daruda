use super::super::look::HomeLook;
use super::*;
use crate::surface::strings as s;

/// A pane with no status since daruda started reads idle, without an emoji;
/// a known status shows its emoji; only a known link gets a button.
#[test]
fn each_row_reads_its_status_and_links_only_a_known_thread() {
    let mut linked = row("asking", Some(ShownStatus::NeedsYou { since: 1 }));
    linked.thread = Some("1.1".into());
    linked.link = Some("https://x/p1".into());
    let mut unlinked = row("quiet", None);
    unlinked.thread = Some("2.2".into());
    let page = Board::new(vec![linked, unlinked, row("left out", None)], 2, None);
    let look = HomeLook::new(&page);
    let view = look.view();
    assert_eq!(view.rows[0].emoji, Some("bell"));
    assert_eq!(
        view.rows[0].status,
        s::remote_channel::thread_status_needs_you()
    );
    assert_eq!(
        view.rows[0].thread,
        Some((
            s::remote_channel::home_open_thread().as_str(),
            "https://x/p1"
        ))
    );
    assert_eq!(view.rows[1].emoji, None);
    assert_eq!(view.rows[1].status, s::remote_channel::home_status_idle());
    assert!(view.rows[1].thread.is_none(), "no link known yet");
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
    let stops: Vec<bool> = view.rows.iter().map(|row| row.stop.is_some()).collect();
    assert_eq!(stops, [true, true, false, false, false]);
    for (index, row) in view.rows.iter().enumerate() {
        assert_eq!(row.select.0, s::remote_channel::home_select());
        assert_eq!(
            row.select.1,
            page.token(index, super::super::token::Act::Select)
        );
    }
    assert_eq!(
        view.rows[0].stop.map(|(label, _)| label.to_owned()),
        Some(s::remote_channel::home_stop())
    );
    let marked: Vec<bool> = view.rows.iter().map(|row| row.selected.is_some()).collect();
    assert_eq!(marked, [false, false, true, false, false]);
    assert_eq!(
        view.outcome,
        Some(s::remote_channel::home_result(chrono::DateTime::UNIX_EPOCH, "Stopped").as_str())
    );
    assert!(
        HomeLook::new(&board(&["a"])).view().outcome.is_none(),
        "no tap yet"
    );
}
