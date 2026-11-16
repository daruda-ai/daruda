//! UI navigation after a successful desktop start; the process still owns the run.

use super::detail::{FlowDetail, FlowDetailBody, FlowDetailId, FlowReturn};
use crate::workspace::{Workspace, pages::Page};
use daruda_store::project::LaneRef;
use gpui::{Context, Focusable as _, Window};
use std::path::{Path, PathBuf};

pub(in crate::workspace) struct PendingFlowStart {
    lane: LaneRef,
    path: PathBuf,
    detail: Option<FlowDetailId>,
}

impl Workspace {
    pub(in crate::workspace) fn begin_flow_start_navigation(&mut self, lane: LaneRef, path: &Path) {
        self.pages.flows.pending_start =
            (self.active_page() == Some(Page::Flows)).then(|| PendingFlowStart {
                lane,
                path: path.to_path_buf(),
                detail: self.flow_detail_id(),
            });
    }

    pub(in crate::workspace) fn cancel_flow_start_navigation(&mut self) {
        self.pages.flows.pending_start = None;
    }

    pub(in crate::workspace) fn flow_definition_return(&self) -> Option<FlowReturn> {
        let detail = self.pages.flows.detail.as_ref()?;
        match &detail.body {
            FlowDetailBody::Graph(view) => Some(FlowReturn {
                id: detail.id,
                lane: detail.lane,
                view: view.clone(),
                browser: self.flows.browser.state.clone(),
            }),
            FlowDetailBody::Run(run) => run.return_to.clone(),
        }
    }

    pub(in crate::workspace) fn finish_flow_start_navigation(
        &mut self,
        lane: LaneRef,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self.pages.flows.pending_start.take() else {
            return;
        };
        if request.lane != lane
            || request.path != path
            || self.active_page() != Some(Page::Flows)
            || self.flow_detail_id() != request.detail
        {
            return;
        }
        let Some(dir) = self
            .flow_run_in(lane, cx)
            .map(|run| run.view().run_dir().to_path_buf())
        else {
            return;
        };
        let parent = self.flow_definition_return();
        self.show_flow_execution(lane, dir, parent, cx);
        self.window_runtime.focus_handle.focus(window, cx);
    }

    pub(in crate::workspace) fn show_flow_execution(
        &mut self,
        lane: LaneRef,
        dir: PathBuf,
        parent: Option<FlowReturn>,
        cx: &mut Context<Self>,
    ) {
        self.mutate_durable(cx, |ws, cx| {
            ws.install_run_detail(lane, dir, cx);
            if let Some(FlowDetail {
                body: FlowDetailBody::Run(run),
                ..
            }) = &mut ws.pages.flows.detail
            {
                run.execution.source_only = parent.is_some();
                run.return_to = parent;
            }
        });
        cx.notify();
    }

    pub(in crate::workspace) fn restore_flow_definition(
        &mut self,
        parent: FlowReturn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let FlowReturn {
            id,
            lane,
            view,
            browser,
        } = parent;
        self.restore_flow_browser(browser, cx);
        self.mutate_durable(cx, |ws, _| {
            ws.pages.flows.detail = Some(FlowDetail {
                id,
                lane,
                body: FlowDetailBody::Graph(view.clone()),
            })
        });
        self.reveal_page_project(lane.project, cx);
        self.refresh_flow_form_catalog(cx);
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.respawn_flow_watcher(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    // Start/profile and retained draft regressions live in tests::flow::browser.
}
