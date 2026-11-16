//! Flow state owned by [`super::Workspace`]: what the Flows tab browses,
//! and the question modal this window has up.
//! Running flows are not here — the process owns them
//! ([`crate::runs::RunsHost`]), so closing this window leaves them going.
//! Where flow locks live is not here either — lane and project removal read
//! it too (`Workspace::lock_root`).

use gpui::{Context, WeakEntity, Window};

use super::Workspace;
use super::flow_ask_modal::FlowAskModal;
use super::flow_browser::FlowBrowser;

pub(in crate::workspace) struct FlowContext {
    pub(in crate::workspace) browser: FlowBrowser,
    /// The question modal this window raised. Weak: the dialog owns it, so
    /// once the dialog closes this no longer upgrades.
    pub(in crate::workspace) ask_modal: Option<WeakEntity<FlowAskModal>>,
    /// The lanes whose flow directories the watcher covers, as
    /// `respawn_flow_watcher` last anchored it; its one writer.
    pub(in crate::workspace) watched_lanes: Vec<daruda_store::project::LaneRef>,
}

impl FlowContext {
    pub(in crate::workspace) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            browser: FlowBrowser::new(window, cx),
            ask_modal: None,
            watched_lanes: Vec::new(),
        }
    }
}
