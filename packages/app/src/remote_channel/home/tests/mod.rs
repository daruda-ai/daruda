//! Home page tests, one file per part: arranging the board and keeping the
//! page up to date.

use super::board::{Board, PaneRow};
use crate::remote_channel::threads::Scope;
use crate::remote_channel::threads::status::ShownStatus;

mod board;
mod look;
mod publish_queue;

pub(super) fn row(name: &str, shown: Option<ShownStatus>) -> PaneRow {
    PaneRow {
        name: name.into(),
        shown,
        thread: None,
        link: None,
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
    Board::new(names.iter().map(|name| row(name, None)).collect(), 10)
}
