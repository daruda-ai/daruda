//! The host's one entry point for what its agent panes do: each report goes
//! to the pane threads, which own the panes' status, then to the app home,
//! which reads it. Home is told here so the threads never call it.

use super::bridge::PaneRef;
use super::threads::RootName;
use super::threads::{roots, status::PaneActivity};
use gpui::App;

/// Report `pane`'s activity. A busy pane reports on every pulse tick; only a
/// changed status reaches the home.
pub(crate) fn activity(pane: PaneRef, activity: PaneActivity, cx: &mut App) {
    if roots::pane_activity(pane, activity, cx) {
        super::home::driver::changed(cx);
    }
}

/// Give `pane` its latest name.
pub(crate) fn named(pane: PaneRef, name: &RootName, cx: &mut App) {
    if roots::pane_named(pane, name, cx) {
        super::home::driver::changed(cx);
    }
}

#[cfg(test)]
mod tests;
