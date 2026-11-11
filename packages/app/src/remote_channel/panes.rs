//! The host's one entry point for what its agent panes do: each report goes
//! to the pane threads, which own the panes' status, then to the app home,
//! which reads it. Home is told here so the threads never call it.

use super::bridge::PaneRef;
use super::threads::RootName;
use super::threads::{roots, status::PaneActivity};
use gpui::App;

/// An agent pane opened. It has reported nothing yet, so only the app home,
/// which lists every pane, has something new to show.
pub(crate) fn opened(cx: &mut App) {
    super::home::driver::changed(cx);
}

/// Report `pane`'s activity. A busy pane reports on every pulse tick; only a
/// changed status reaches the home.
pub(crate) fn activity(pane: PaneRef, activity: PaneActivity, cx: &mut App) {
    if roots::pane_activity(pane, activity, cx) {
        super::home::driver::changed(cx);
    }
}

/// Give `pane` its latest name. The home is told every time: the threads
/// know names only of panes that have a thread, while the home lists every
/// pane. Reported once a turn ends, and a rebuild only runs while a page is
/// viewed and sends nothing when the board is unchanged.
pub(crate) fn named(pane: PaneRef, name: &RootName, cx: &mut App) {
    roots::pane_named(pane, name, cx);
    super::home::driver::changed(cx);
}

#[cfg(test)]
mod tests;
