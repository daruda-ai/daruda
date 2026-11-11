//! How a board reads on the page: every text in the person's language, held
//! so the transport can borrow it. One builder for every publish.

use super::board::Board;
use super::token::Act;
use crate::remote_channel::threads::status::ShownStatus;
use crate::remote_channel::transport::{PageRow, PageView};
use crate::surface::strings as s;

pub(super) struct HomeLook {
    header: String,
    outcome: Option<String>,
    empty: String,
    rows: Vec<RowLook>,
    more: Option<String>,
    open_thread: String,
    select: String,
    stop: String,
    selected: String,
}

struct RowLook {
    name: String,
    emoji: Option<&'static str>,
    status: String,
    link: Option<String>,
    selected: bool,
    select: String,
    /// Only a running pane has something to stop.
    stop: Option<String>,
}

impl HomeLook {
    pub(super) fn new(board: &Board) -> Self {
        Self {
            header: s::remote_channel::home_header(),
            outcome: board.outcome.as_ref().map(|outcome| {
                s::remote_channel::home_result(
                    chrono::DateTime::from_timestamp(outcome.at, 0).unwrap_or_default(),
                    &outcome.text,
                )
            }),
            empty: s::control::listing_empty(),
            rows: board
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let (emoji, status) = match row.shown.map(ShownStatus::view) {
                        Some((emoji, label)) => (Some(emoji), label),
                        None => (None, s::remote_channel::home_status_idle()),
                    };
                    RowLook {
                        name: row.name.clone(),
                        emoji,
                        status,
                        link: row.link.clone(),
                        selected: row.selected,
                        select: board.token(index, Act::Select),
                        stop: row
                            .shown
                            .is_some_and(ShownStatus::is_running)
                            .then(|| board.token(index, Act::Stop)),
                    }
                })
                .collect(),
            more: (board.omitted > 0).then(|| s::remote_channel::home_more(board.omitted)),
            open_thread: s::remote_channel::home_open_thread(),
            select: s::remote_channel::home_select(),
            stop: s::remote_channel::home_stop(),
            selected: s::remote_channel::home_selected(),
        }
    }

    pub(super) fn view(&self) -> PageView<'_> {
        PageView {
            header: &self.header,
            outcome: self.outcome.as_deref(),
            empty: &self.empty,
            rows: self
                .rows
                .iter()
                .map(|row| PageRow {
                    name: &row.name,
                    selected: row.selected.then_some(self.selected.as_str()),
                    emoji: row.emoji,
                    status: &row.status,
                    thread: row
                        .link
                        .as_deref()
                        .map(|link| (self.open_thread.as_str(), link)),
                    select: (self.select.as_str(), row.select.as_str()),
                    stop: row.stop.as_deref().map(|token| (self.stop.as_str(), token)),
                })
                .collect(),
            more: self.more.as_deref(),
        }
    }
}
