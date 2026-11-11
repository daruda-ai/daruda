//! How a board reads on the page: every text in the person's language, held
//! so the transport can borrow it. One builder for every publish.

use super::board::Board;
use crate::remote_channel::threads::status::ShownStatus;
use crate::remote_channel::transport::slack::home::{HomeView, RowView};
use crate::surface::strings as s;

pub(super) struct HomeLook {
    header: String,
    empty: String,
    rows: Vec<RowLook>,
    more: Option<String>,
    open_thread: String,
}

struct RowLook {
    name: String,
    emoji: Option<&'static str>,
    status: String,
    link: Option<String>,
}

impl HomeLook {
    pub(super) fn new(board: &Board) -> Self {
        Self {
            header: s::remote_channel::home_header(),
            empty: s::control::listing_empty(),
            rows: board
                .rows
                .iter()
                .map(|row| {
                    let (emoji, status) = match row.shown.map(ShownStatus::view) {
                        Some((emoji, label)) => (Some(emoji), label),
                        None => (None, s::remote_channel::home_status_idle()),
                    };
                    RowLook {
                        name: row.name.clone(),
                        emoji,
                        status,
                        link: row.link.clone(),
                    }
                })
                .collect(),
            more: (board.omitted > 0).then(|| s::remote_channel::home_more(board.omitted)),
            open_thread: s::remote_channel::home_open_thread(),
        }
    }

    pub(super) fn view(&self) -> HomeView<'_> {
        HomeView {
            header: &self.header,
            empty: &self.empty,
            rows: self
                .rows
                .iter()
                .map(|row| RowView {
                    name: &row.name,
                    emoji: row.emoji,
                    status: &row.status,
                    thread: row
                        .link
                        .as_deref()
                        .map(|link| (self.open_thread.as_str(), link)),
                })
                .collect(),
            more: self.more.as_deref(),
        }
    }
}
