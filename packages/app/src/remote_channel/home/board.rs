//! What a page lists: the panes, the ones the person is needed by or that are
//! working first, cut to what the page has room for. GPUI-free.

use super::token::{Act, Numbering, Tap};
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::command::{ListingRow, agent_label, title_suffix};
use crate::remote_channel::threads::status::ShownStatus;
use crate::surface::strings as s;

/// One pane on the page, named as its thread root names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PaneRow {
    pub pane: PaneRef,
    pub name: String,
    /// What the pane is working on, as its activity bar says.
    pub subject: Option<String>,
    /// `None` while the pane has not reported since daruda started.
    pub shown: Option<ShownStatus>,
    /// The root of the pane's thread on this connection, when it has one.
    pub thread: Option<String>,
    /// The link that opens that thread, once known.
    pub link: Option<String>,
    /// The pane the connection's commands address.
    pub selected: bool,
}

impl PaneRow {
    /// A listing row as `/list` names it: the name a row falls back to
    /// when its workspace cannot name the pane.
    pub(super) fn named(row: &ListingRow) -> String {
        s::remote_channel::home_row_name(
            &row.name,
            agent_label(&row.summary),
            title_suffix(&row.summary),
        )
    }
}

/// What the person's last tap on the page came to, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    /// Unix seconds.
    pub at: i64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Board {
    rows: Vec<PaneRow>,
    /// Panes left out for want of room.
    omitted: usize,
    /// What the rows' buttons carry, so a tap names this board's row.
    generation: u64,
    outcome: Option<Outcome>,
}

impl Board {
    /// `rows` in listing order, arranged for a page of `capacity` rows, under
    /// what the person's last tap came to.
    pub(super) fn new(mut rows: Vec<PaneRow>, capacity: usize, outcome: Option<Outcome>) -> Self {
        // Stable: within a rank, panes keep their listing order.
        rows.sort_by_key(|row| rank(row.shown));
        let omitted = rows.len().saturating_sub(capacity);
        rows.truncate(capacity);
        Self {
            rows,
            omitted,
            generation: 0,
            outcome,
        }
    }

    pub(super) fn rows(&self) -> &[PaneRow] {
        &self.rows
    }

    /// Panes left out for want of room.
    pub(super) fn omitted(&self) -> usize {
        self.omitted
    }

    /// What the person's last tap came to, shown atop the rows.
    pub(super) fn outcome(&self) -> Option<&Outcome> {
        self.outcome.as_ref()
    }

    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Give the rows' buttons the generation `numbering` has for them.
    pub(super) fn number(&mut self, numbering: &mut Numbering) {
        self.generation = numbering.number(self.rows.iter().map(|row| row.pane).collect());
    }

    /// The value of row `index`'s button for `act`.
    pub(super) fn token(&self, index: usize, act: Act) -> String {
        Tap {
            generation: self.generation,
            index,
            act,
        }
        .token()
    }

    /// The pane `tap` names, when it was made for this board.
    pub(super) fn resolve(&self, tap: Tap) -> Option<PaneRef> {
        (tap.generation == self.generation)
            .then(|| self.rows.get(tap.index).map(|row| row.pane))
            .flatten()
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
