//! The app's own page on channels that have one: every agent pane, its
//! status, and a link to its thread. Shown when the paired person opens it;
//! a row's buttons select its pane or stop it.

mod board;
pub(crate) mod driver;
mod look;
mod publish_queue;
mod token;

use crate::remote_channel::threads::Scope;
use board::{Board, WindowPanes};

/// Every connection's page, and the numbering their buttons share.
#[derive(Default)]
pub(crate) struct Home {
    queue: publish_queue::PublishQueue,
    /// A rebuild of the viewed pages waits for the end of the current
    /// update, so the changes of one frame make one rebuild.
    rebuild_pending: bool,
    /// The generation every board's buttons carry.
    numbering: token::Numbering,
}

impl Home {
    /// A pane changed: whether a rebuild must be scheduled for it. Only when
    /// the person views a page and none is scheduled yet.
    fn schedule_rebuild(&mut self) -> bool {
        if self.rebuild_pending || self.queue.viewing().next().is_none() {
            return false;
        }
        self.rebuild_pending = true;
        true
    }

    /// The scheduled rebuild runs now: a later change schedules another.
    fn start_rebuild(&mut self) {
        self.rebuild_pending = false;
    }

    /// The board `scope`'s page shows next: `windows`' panes laid out for
    /// `budget`, under what the last tap came to, with the thread links the
    /// page knows, and buttons numbered for this board.
    fn board(
        &mut self,
        scope: &Scope,
        windows: Vec<WindowPanes>,
        budget: crate::remote_channel::transport::PageBudget,
    ) -> Board {
        let mut board = Board::new(windows, budget, self.queue.outcome(scope).cloned());
        self.queue.link(scope, &mut board);
        board.number(&mut self.numbering);
        board
    }
}

/// What a test outside this module reads of a page.
#[cfg(test)]
impl Home {
    /// The value of row `pane`'s stop button, or its select button, on the
    /// board `scope`'s page is about to show or shows.
    pub(crate) fn button_for_test(
        &self,
        scope: &Scope,
        pane: crate::remote_channel::bridge::PaneRef,
        stop: bool,
    ) -> Option<String> {
        let board = self.queue.board(scope)?;
        let index = board.rows().iter().position(|row| row.pane == pane)?;
        let act = if stop {
            token::Act::Stop
        } else {
            token::Act::Select
        };
        Some(board.token(index, act))
    }

    /// Whether a rebuild of the viewed pages waits for the current update
    /// to end.
    pub(crate) fn rebuild_scheduled_for_test(&self) -> bool {
        self.rebuild_pending
    }

    /// What the last tap came to on the board `scope`'s page shows, and
    /// whether it shows one.
    pub(crate) fn shown_outcome_for_test(&self, scope: &Scope) -> Option<Option<String>> {
        let board = self.queue.published(scope)?;
        Some(board.outcome().map(|outcome| outcome.text.clone()))
    }

    /// What the last tap on `scope`'s page came to.
    pub(crate) fn outcome_for_test(&self, scope: &Scope) -> Option<String> {
        self.queue
            .outcome(scope)
            .map(|outcome| outcome.text.clone())
    }
}

#[cfg(test)]
mod tests;
