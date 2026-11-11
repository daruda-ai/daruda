//! The app's own page on channels that have one: every agent pane, its
//! status, and a link to its thread. Shown when the paired person opens it;
//! read-only, since nothing on it acts on a pane.

mod board;
pub(crate) mod driver;
mod look;
mod publish_queue;

use std::collections::HashMap;

/// Every connection's page and the thread links it has learned.
#[derive(Default)]
pub(crate) struct Home {
    queue: publish_queue::PublishQueue,
    /// The link to each thread root, by connection and root. A message's link
    /// does not change, so it is asked for once a run.
    links: HashMap<(String, String), String>,
    /// A rebuild of the viewed pages waits for the end of the current
    /// update, so the changes of one frame make one rebuild.
    rebuild_pending: bool,
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
}

#[cfg(test)]
mod tests;
