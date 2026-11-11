//! One connection's page under one identity: who views it, the board it
//! should show next, the one it shows, and where the publish between them
//! stands. `PublishQueue` keeps a page per connection.

use super::super::board::{Board, Outcome};
use super::super::token::Tap;
use super::links::PageLinks;
use crate::remote_channel::bridge::PaneRef;
use crate::remote_channel::threads::Scope;
use crate::remote_channel::transport::PageVerdict;

/// Whether the person has a page daruda should keep up to date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Viewer {
    /// Not opened since daruda started: nothing is published unasked.
    NotSeen,
    Viewing,
    /// The channel turned the page off; nothing is published until the
    /// person opens it again.
    Unavailable,
}

/// Where a page's next publish stands, when one is owed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// The board on its way, which may already be on screen.
    InFlight(Board),
    /// Failed for a passing reason, or its connection could not send; built
    /// anew on the retry tick or the person's next opening.
    Waiting,
}

/// Which page, of all a connection had, an ask was made for: a page another
/// identity replaced, even one under the same identity again, has another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::remote_channel::home) struct PageEpoch(pub(super) u64);

pub(super) struct Page {
    pub scope: Scope,
    /// Tells this page's link answers from a replaced page's.
    pub epoch: PageEpoch,
    pub viewer: Viewer,
    wanted: Option<Board>,
    /// What the page shows now, as far as this run knows.
    published: Option<Board>,
    phase: Option<Phase>,
    /// The board the channel refused last: an equal board would be refused
    /// again, so it is not sent until another has gone out.
    refused: Option<Board>,
    /// What the person's last tap came to, shown atop every later board.
    pub outcome: Option<Outcome>,
    pub links: PageLinks,
}

impl Page {
    pub(super) fn new(scope: Scope, epoch: PageEpoch) -> Self {
        Self {
            scope,
            epoch,
            viewer: Viewer::NotSeen,
            wanted: None,
            published: None,
            phase: None,
            refused: None,
            outcome: None,
            links: PageLinks::default(),
        }
    }

    /// Give up what waits for a later trigger: a board that could not go out
    /// is built anew, since the panes have moved on meanwhile, and links that
    /// failed for a passing reason are asked again. Whether there was
    /// anything to give up.
    pub(super) fn resume(&mut self) -> bool {
        let waiting = self.phase == Some(Phase::Waiting);
        if waiting {
            self.phase = None;
            self.wanted = None;
        }
        self.links.retry_failed() | waiting
    }

    /// The pane `tap` names: on the board shown, on the one in flight, or on
    /// the one wanted next; either of the last two may already be on screen.
    pub(super) fn resolve(&self, tap: Tap) -> Option<PaneRef> {
        let in_flight = match &self.phase {
            Some(Phase::InFlight(board)) => Some(board),
            _ => None,
        };
        [self.published.as_ref(), in_flight, self.wanted.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|board| board.resolve(tap))
    }

    /// Show `board` next, if the person is viewing the page.
    pub(super) fn want(&mut self, board: Board) {
        if self.viewer == Viewer::Viewing {
            self.wanted = Some(board);
        }
    }

    /// The board to publish now, marked in flight: wanted, not already
    /// shown, not refused last, not in flight, and not waiting.
    pub(super) fn ready(&mut self) -> Option<Board> {
        // With a publish in flight, what the page will show is not known
        // until it settles; the wanted board is compared then.
        let in_flight = matches!(self.phase, Some(Phase::InFlight(_)));
        if !in_flight && self.wanted.is_some() && self.wanted == self.published {
            self.wanted = None;
        }
        if self.wanted.is_some() && self.wanted == self.refused {
            self.wanted = None;
        }
        if self.viewer != Viewer::Viewing || self.phase.is_some() {
            return None;
        }
        let board = self.wanted.clone()?;
        self.refused = None;
        self.phase = Some(Phase::InFlight(board.clone()));
        Some(board)
    }

    /// Record that publishing `board` came to `verdict`.
    pub(super) fn finish(&mut self, board: &Board, verdict: &PageVerdict) {
        self.phase = None;
        match verdict {
            PageVerdict::Published => self.published = Some(board.clone()),
            PageVerdict::Off => {
                self.viewer = Viewer::Unavailable;
                self.wanted = None;
                // The channel may not keep the old view across the page
                // going off and on, so the next opening publishes again.
                self.published = None;
            }
            PageVerdict::Transient(_) => self.phase = Some(Phase::Waiting),
            PageVerdict::Refused(_) => {
                if self.wanted.as_ref() == Some(board) {
                    self.wanted = None;
                }
                self.refused = Some(board.clone());
            }
        }
    }

    /// Keep the board in flight waiting: its connection cannot send now.
    pub(super) fn hold(&mut self) {
        self.phase = Some(Phase::Waiting);
    }

    /// The board the page is about to show, or else shows.
    #[cfg(test)]
    pub(super) fn board(&self) -> Option<&Board> {
        self.wanted.as_ref().or(self.published.as_ref())
    }

    /// The board the page shows, as far as this run knows.
    #[cfg(test)]
    pub(super) fn published(&self) -> Option<&Board> {
        self.published.as_ref()
    }
}
