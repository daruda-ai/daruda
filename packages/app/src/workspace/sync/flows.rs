//! Flow-definition watcher → GPUI bridge.
//!
//! Mirrors [`super::mcp`]: every 100 ms the pump drains whatever the watcher
//! debounced and hands it to [`Workspace::apply_flows_event`]. Dropping the
//! returned `Task<()>` (held as `_flow_event_pump` on `Workspace`) cancels the
//! loop, the receiver disconnects, and the watcher thread exits.
//!
//! A change names directories, not files — see
//! [`crate::hooks::flow_watcher::FlowsEvent`]: the open graph reads its file
//! again, and does nothing at all when its bytes did not change, while only
//! the listings that read one of those directories are read again.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use gpui::{Context, Task, Window};

use crate::hooks::flow_watcher::{self, FlowsEvent};
use crate::workspace::lifetimes::Watch;
use crate::workspace::{Workspace, flow_paths};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) fn spawn(events: Receiver<FlowsEvent>, cx: &mut Context<Workspace>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        'outer: loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            loop {
                let event = match events.try_recv() {
                    Ok(event) => event,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break 'outer,
                };
                // A window is needed because rebuilding a graph builds a canvas,
                // which is a view. `update_in` fails once the window is gone —
                // that is the same signal as a released entity, so stop.
                if this
                    .update_in(cx, |ws, window, cx| {
                        ws.apply_flows_event(&event, window, cx)
                    })
                    .is_err()
                {
                    break 'outer;
                }
            }
        }
    })
}

impl Workspace {
    /// Something in a watched flow directory changed: forget the cached lists
    /// that read it, and let the open graph re-read its file.
    pub(in crate::workspace) fn apply_flows_event(
        &mut self,
        event: &FlowsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FlowsEvent::In(dirs) => self.invalidate_flow_lists_in(dirs),
            FlowsEvent::Unknown => self.invalidate_every_flow_list(),
        }
        self.reload_flow_graphs(None, window, cx);
        // No blanket notify: a graph that actually changed raised its own, which
        // marks this view dirty through it, and gpui has no partial redraw — a
        // notify here would repaint the whole window for an event that changed
        // nothing (an editor touching a file, our own write coming back). What
        // it *is* needed for is the panel's list, which is rendered from here.
        if self.active_page() == Some(crate::workspace::pages::Page::Flows)
            && self.shown_page_detail_id().is_none()
        {
            cx.notify();
        }
    }

    /// Watch active and browsed flow sources, and the open graph's. Re-anchor after scope changes
    /// or file creation, which may create a previously absent directory.
    ///
    /// A lane's cached listing is kept right only while its directories are
    /// watched. One that comes back under the watch may have missed changes
    /// meanwhile, so its listing and history are read again.
    pub(in crate::workspace) fn respawn_flow_watcher(&mut self, cx: &mut Context<Self>) {
        self.pumps.flow = None;

        let mut lanes = Vec::new();
        if self.flow_sources().is_some() {
            lanes.push(self.active);
            if self.active_page() == Some(crate::workspace::pages::Page::Flows) {
                lanes.extend(self.flow_browser_lanes());
            }
            // The graph's own lane: the page may have moved on to browse another.
            if let Some((_, lane, _)) = self.open_graph() {
                lanes.push(lane);
            }
        }
        lanes.sort_by_key(|lane| (lane.project, lane.lane));
        lanes.dedup();
        let previous = std::mem::take(&mut self.flows.watched_lanes);
        for lane in lanes.iter().filter(|lane| !previous.contains(lane)) {
            if let Some(state) = self.lane_scoped.get_mut(lane) {
                state.flows.list.invalidate();
                state.flows.history.invalidate();
            }
        }
        // The origin is what a *listing* needs; anchoring only needs the paths.
        let mut dirs: Vec<_> = lanes
            .iter()
            .filter_map(|lane| self.flow_sources_for(*lane))
            .flat_map(|sources| sources.dirs().into_iter().map(|(dir, _)| dir))
            .collect();
        dirs.sort();
        dirs.dedup();
        self.flows.watched_lanes = lanes;
        if dirs.is_empty() {
            return;
        }
        let (events, handle) = flow_watcher::spawn(dirs, flow_paths::FLOW_EXTENSIONS.to_vec());
        self.pumps.flow = Some(Watch::new(handle, spawn(events, cx)));
    }
}
