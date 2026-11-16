//! Browsing dispatches bind a worktree before any dialog or editor is opened.

use std::path::Path;

use daruda_store::project::LaneRef;
use gpui::{App, Context, Window};

use super::{FlowBrowserSnapshot, FlowGrouping, FlowScope, FlowTab, FlowTarget, RunFilter};
use crate::surface::strings as s;
use crate::workspace::{
    Workspace, flow_paths::FlowOrigin, layout::diff_policy::Handle, pages::Page,
};

impl Workspace {
    pub(in crate::workspace) fn stage_flow_page(&mut self, cx: &Context<Self>) {
        // Only while the list is what the page draws: reading the browser's
        // inputs for a page showing its graph would re-arm their repaints
        // for nothing on screen (render-cost rule 10).
        let list_shown =
            self.active_page() == Some(Page::Flows) && self.shown_page_detail_id().is_none();
        self.flows.browser.page_snapshot = if list_shown {
            let groups = self
                .flow_browser_lanes()
                .into_iter()
                .map(|lane| self.flow_lane_snapshot(lane, cx))
                .collect();
            Some(super::FlowPageSnapshot {
                workspace: cx.weak_entity(),
                scope: self.scope_snapshot(),
                groups,
                flow_browser: self.flow_browser_snapshot(cx),
            })
        } else {
            None
        };
    }

    /// One worktree's group: what its caches hold, with a read started off
    /// the main thread for whichever is missing or wrong. The old values stay
    /// until those reads land.
    fn flow_lane_snapshot(&mut self, lane: LaneRef, cx: &Context<Self>) -> super::FlowLaneSnapshot {
        self.read_flow_list(lane, cx);
        self.read_flow_history(lane, cx);
        let state = self.lane_scoped.get(&lane);
        let listing = state.and_then(|state| state.flows.list.shown(lane));
        super::FlowLaneSnapshot {
            lane,
            label: self.lane_label_for(lane),
            flows: self.flow_rows_matching(|target| target == lane, cx),
            history: state
                .and_then(|state| state.flows.history.shown(lane))
                .cloned(),
            files: listing.map(|list| list.files.clone()).unwrap_or_default(),
            modified: listing
                .map(|list| list.modified.clone())
                .unwrap_or_default(),
        }
    }

    /// The browsed worktree's group, staged the way the page stages it —
    /// nothing while the Flows page is hidden or that worktree is gone.
    #[cfg(test)]
    fn browsed_flow_group(&mut self, cx: &Context<Self>) -> Option<super::FlowLaneSnapshot> {
        if self.active_page() != Some(Page::Flows) {
            return None;
        }
        let lane = self.flow_browser_lane();
        self.lane_for(lane)?;
        Some(self.flow_lane_snapshot(lane, cx))
    }

    #[cfg(test)]
    pub(in crate::workspace) fn flow_list_for_panel(
        &mut self,
        cx: &Context<Self>,
    ) -> Vec<crate::workspace::flow_paths::FoundFlow> {
        self.browsed_flow_group(cx)
            .map(|group| group.files)
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::workspace) fn flow_history_for_panel(
        &mut self,
        cx: &Context<Self>,
    ) -> Option<crate::workspace::flow_history::FlowHistory> {
        self.browsed_flow_group(cx)?.history
    }

    pub(in crate::workspace) fn flow_browser_lanes(&self) -> Vec<LaneRef> {
        self.projects
            .lanes()
            .filter(|(lane, _, _)| self.page_scope_matches(lane.project))
            .filter(|(lane, _, _)| match self.flows.browser.state.scope {
                FlowScope::All => true,
                FlowScope::Worktree(selected) => *lane == selected,
            })
            .map(|(lane, _, _)| lane)
            .collect()
    }

    /// Single-target commands bind a concrete worktree; aggregate creation
    /// uses the explicit target menu instead of this fallback.
    #[cfg(test)]
    pub(in crate::workspace) fn flow_browser_lane(&self) -> LaneRef {
        self.flow_browser_lanes()
            .into_iter()
            .find(|lane| *lane == self.active)
            .or_else(|| self.flow_browser_lanes().into_iter().next())
            .unwrap_or(self.active)
    }

    pub(in crate::workspace) fn flow_browser_snapshot(&self, cx: &App) -> FlowBrowserSnapshot {
        let search = &self.flows.browser.searches[self.flows.browser.state.tab.index()];
        FlowBrowserSnapshot {
            state: self.flows.browser.state.clone(),
            targets: self
                .projects
                .lanes()
                .filter(|(lane, _, _)| self.page_scope_matches(lane.project))
                .map(|(target, project, _)| FlowTarget {
                    lane: target,
                    project: project.name.clone(),
                    label: self.lane_label_for(target),
                    current: target == self.active,
                })
                .collect(),
            search: Handle(search.clone()),
            query: search.read(cx).value().to_string(),
        }
    }

    pub(in crate::workspace) fn set_flow_scope(
        &mut self,
        scope: FlowScope,
        cx: &mut Context<Self>,
    ) {
        if let FlowScope::Worktree(lane) = scope {
            let Some(project) = self.project_for(lane.project) else {
                return;
            };
            let project = project.uuid;
            if self.lane_for(lane).is_none() {
                return;
            }
            if !self.page_scope_matches(lane.project) {
                self.set_page_scope(
                    crate::workspace::pages::scope::ProjectScope::Project(project),
                    cx,
                );
            }
        }
        self.cancel_flow_start_navigation();
        self.flows.browser.state.scope = scope;
        self.pages.flows.history_request = None;
        self.respawn_flow_watcher(cx);
        self.reset_flow_scroll();
        cx.notify();
    }

    pub(in crate::workspace) fn restore_flow_browser(
        &mut self,
        state: super::FlowBrowserState,
        cx: &mut Context<Self>,
    ) {
        let tab = state.tab;
        self.flows.browser.state = state;
        self.workspace_page
            .set_scroll(Page::Flows, self.flows.browser.scrolls[tab.index()].clone());
        self.respawn_flow_watcher(cx);
        cx.notify();
    }

    pub(in crate::workspace) fn set_flow_tab(&mut self, tab: FlowTab, cx: &mut Context<Self>) {
        if self.flows.browser.state.tab != tab {
            self.cancel_flow_start_navigation();
            self.pages.flows.history_request = None;
        }
        self.flows.browser.state.tab = tab;
        self.workspace_page
            .set_scroll(Page::Flows, self.flows.browser.scrolls[tab.index()].clone());
        cx.notify();
    }

    pub(in crate::workspace) fn set_flow_origin(
        &mut self,
        origin: Option<FlowOrigin>,
        cx: &mut Context<Self>,
    ) {
        self.flows.browser.state.origin = origin;
        self.reset_flow_scroll();
        cx.notify();
    }

    pub(in crate::workspace) fn set_flow_run_filter(
        &mut self,
        filter: RunFilter,
        cx: &mut Context<Self>,
    ) {
        self.flows.browser.state.run_filter = filter;
        self.reset_flow_scroll();
        cx.notify();
    }

    pub(in crate::workspace) fn set_flow_grouping(
        &mut self,
        grouping: FlowGrouping,
        cx: &mut Context<Self>,
    ) {
        self.flows.browser.state.grouping = grouping;
        cx.notify();
    }

    pub(in crate::workspace) fn toggle_flow_group(
        &mut self,
        origin: FlowOrigin,
        cx: &mut Context<Self>,
    ) {
        self.flows.browser.state.toggle_group(origin);
        cx.notify();
    }

    fn reset_flow_scroll(&self) {
        self.flows.browser.scrolls[self.flows.browser.state.tab.index()]
            .set_offset(gpui::point(gpui::Pixels::ZERO, gpui::Pixels::ZERO));
    }

    pub(in crate::workspace) fn clear_flow_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.flows.browser.searches[self.flows.browser.state.tab.index()].update(
            cx,
            |input, cx| {
                input.set_value("", window, cx);
            },
        );
        self.reset_flow_scroll();
        cx.notify();
    }

    pub(in crate::workspace) fn clear_flow_filters(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.flows.browser.state.tab {
            FlowTab::Definitions => self.flows.browser.state.origin = None,
            FlowTab::Runs => self.flows.browser.state.run_filter = RunFilter::All,
        }
        self.clear_flow_search(window, cx);
    }

    pub(in crate::workspace) fn show_flow_questions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_flow_tab(FlowTab::Runs, cx);
        self.set_flow_run_filter(RunFilter::Asking, cx);
        self.clear_flow_search(window, cx);
    }

    pub(in crate::workspace) fn prompt_new_flow(
        &mut self,
        lane: LaneRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lane_for(lane).is_none() {
            return;
        }
        self.create_flow_in(lane, &s::flow::untitled(), window, cx);
    }

    /// Open the graph in the page, run in the worktree the page browses —
    /// the active one stays as it is.
    pub(in crate::workspace) fn open_browsed_flow(
        &mut self,
        lane: LaneRef,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lane_for(lane).is_none() {
            return;
        }
        self.open_flow_graph(lane, path, window, cx);
    }

    pub(in crate::workspace) fn edit_browsed_flow_name(
        &mut self,
        lane: LaneRef,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.lane_for(lane).is_none() {
            return;
        }
        self.open_flow_graph_then(lane, path, window, cx, |_, view, window, cx| {
            view.update(cx, |view, cx| view.focus_name(window, cx));
        });
    }

    pub(in crate::workspace) fn run_browsed_flow(
        &mut self,
        lane: LaneRef,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = path.to_path_buf();
        self.leave_page_detail_then(Page::Flows, window, cx, move |ws, window, cx| {
            ws.begin_flow_start_navigation(lane, &path);
            let accepted = ws.run_flow_at(
                lane,
                &path,
                crate::workspace::command::flow_picker::FlowPurpose::Run,
                crate::workspace::flow_request::FlowSelection::default(),
                window,
                cx,
            );
            if !accepted {
                ws.cancel_flow_start_navigation();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scope_labels_keep_project_and_worktree_identity() {
        let state = super::FlowTarget {
            lane: super::LaneRef {
                project: 1,
                lane: 0,
            },
            project: "Project".into(),
            label: "Project / main".into(),
            current: true,
        };
        assert!(state.label.contains(&state.project));
    }
}
