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
}

#[cfg(test)]
mod tests;
