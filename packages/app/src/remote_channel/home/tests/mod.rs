//! Home page tests, one file per part: arranging the board and keeping the
//! page up to date.

use super::board::{Board, PaneRow, WindowPanes};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::threads::status::ShownStatus;
use crate::remote_channel::transport::PageBudget;
use crate::workspace::{ArrangedLane, ArrangedProject, ArrangedSection};

mod board;
mod driver;
mod look;
mod page_links;
mod publish_queue;

pub(super) fn row(name: &str, shown: Option<ShownStatus>) -> PaneRow {
    PaneRow {
        pane: pane(name.bytes().map(u64::from).sum()),
        name: name.into(),
        subject: None,
        shown,
        thread: None,
        link: None,
        selected: false,
    }
}

pub(super) fn pane(pane: u64) -> PaneRef {
    PaneRef {
        workspace: Default::default(),
        pane,
    }
}

pub(super) fn scope(connection: &str, bot: &str) -> Scope {
    Scope {
        connection: connection.into(),
        bot: bot.into(),
        conversation: "D1".into(),
    }
}

pub(super) fn board(names: &[&str]) -> Board {
    flat(names.iter().map(|name| row(name, None)).collect())
}

/// Room for every board a test builds but the ones about room.
pub(super) const ROOMY: PageBudget = PageBudget {
    blocks: 1000,
    cards_per_block: 10,
    cards: 1000,
};

/// `rows` as one window's one ungrouped project with one lane, in order.
pub(super) fn flat(rows: Vec<PaneRow>) -> Board {
    Board::new(
        vec![window(
            1,
            vec![alone(project("p", vec![lane("main", rows)]))],
        )],
        ROOMY,
        None,
    )
}

pub(super) fn window(number: usize, sections: Vec<ArrangedSection<PaneRow>>) -> WindowPanes {
    WindowPanes { number, sections }
}

pub(super) fn group(
    name: &str,
    projects: Vec<ArrangedProject<PaneRow>>,
) -> ArrangedSection<PaneRow> {
    ArrangedSection::Group {
        name: name.into(),
        projects,
    }
}

pub(super) fn alone(project: ArrangedProject<PaneRow>) -> ArrangedSection<PaneRow> {
    ArrangedSection::Project(project)
}

pub(super) fn project(name: &str, lanes: Vec<ArrangedLane<PaneRow>>) -> ArrangedProject<PaneRow> {
    ArrangedProject {
        name: name.into(),
        lanes,
    }
}

pub(super) fn lane(name: &str, panes: Vec<PaneRow>) -> ArrangedLane<PaneRow> {
    ArrangedLane {
        name: name.into(),
        panes,
    }
}
