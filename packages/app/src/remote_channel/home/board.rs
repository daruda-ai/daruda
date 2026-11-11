//! What a page lists: the panes, the ones the person is needed by or that are
//! working first, cut to what the page has room for. GPUI-free.

use crate::remote_channel::command::{ListingRow, agent_label, title_suffix};
use crate::remote_channel::threads::status::ShownStatus;
use crate::surface::strings as s;

/// One pane on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneRow {
    pub name: String,
    /// `None` while the pane has not reported since daruda started.
    pub shown: Option<ShownStatus>,
    /// The root of the pane's thread on this connection, when it has one.
    pub thread: Option<String>,
    /// The link that opens that thread, once known.
    pub link: Option<String>,
}

impl PaneRow {
    /// A listing row as `/list` names it.
    pub(super) fn named(row: &ListingRow) -> String {
        s::remote_channel::home_row_name(
            &row.name,
            agent_label(&row.summary),
            title_suffix(&row.summary),
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Board {
    pub rows: Vec<PaneRow>,
    /// Panes left out for want of room.
    pub omitted: usize,
}

impl Board {
    /// `rows` in listing order, arranged for a page of `capacity` rows.
    pub(super) fn new(mut rows: Vec<PaneRow>, capacity: usize) -> Self {
        // Stable: within a rank, panes keep their listing order.
        rows.sort_by_key(|row| rank(row.shown));
        let omitted = rows.len().saturating_sub(capacity);
        rows.truncate(capacity);
        Self { rows, omitted }
    }

    /// The thread roots shown without a link yet.
    pub(super) fn unlinked(&self) -> impl Iterator<Item = &str> {
        self.rows
            .iter()
            .filter(|row| row.link.is_none())
            .filter_map(|row| row.thread.as_deref())
    }

    /// Give each row the link `link_of` knows for its thread.
    pub(super) fn link(&mut self, link_of: impl Fn(&str) -> Option<String>) {
        for row in &mut self.rows {
            if row.link.is_none() {
                row.link = row.thread.as_deref().and_then(&link_of);
            }
        }
    }
}

/// What the person must act on comes first, then what is still running.
fn rank(shown: Option<ShownStatus>) -> u8 {
    match shown {
        Some(ShownStatus::NeedsYou { .. }) => 0,
        Some(ShownStatus::Working { .. }) => 1,
        Some(ShownStatus::Ended(_) | ShownStatus::Closed) | None => 2,
    }
}
