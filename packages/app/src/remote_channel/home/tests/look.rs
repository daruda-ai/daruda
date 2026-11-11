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
    let page = Board::new(vec![linked, unlinked, row("left out", None)], 2);
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
