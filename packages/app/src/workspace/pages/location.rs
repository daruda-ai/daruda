//! Fixed detail ownership, independent of the list's shared project scope.

use super::{Page, flows::detail::FlowDetailBody};
use crate::{
    surface::strings as s,
    ui::{button_icon, icons, theme},
    workspace::Workspace,
};
use daruda_store::project::LaneRef;
use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};

struct Location {
    owner: String,
    item: String,
    lane: Option<LaneRef>,
    back_id: &'static str,
    back_label: String,
}

impl Workspace {
    fn detail_location(&self, page: Page, cx: &Context<Self>) -> Option<Location> {
        self.shown_page_detail_id()?;
        let (owner, item, lane, back_id, back_label) = match page {
            Page::Tasks => {
                let detail = self.pages.tasks.detail.as_ref()?;
                let lane = detail
                    .editor
                    .task_id
                    .as_ref()
                    .and_then(|id| self.right_views.tasks.worktrees.get(id))
                    .copied();
                (
                    detail
                        .project
                        .and_then(|uuid| self.project_by_uuid(uuid))
                        .map(|project| project.name.clone())
                        .unwrap_or_else(s::task::project_not_open),
                    detail.editor.title().to_string(),
                    lane,
                    "task-edit-back",
                    s::task::edit_back(),
                )
            }
            Page::Flows => {
                let detail = self.pages.flows.detail.as_ref()?;
                let (item, back_id, back_label) = match &detail.body {
                    FlowDetailBody::Graph(view) => (
                        view.read(cx).name().to_owned(),
                        "flow-back-to-list",
                        s::flow::back_to_list(),
                    ),
                    FlowDetailBody::Run(run) => (
                        run.dir
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(s::flow::run_history_title),
                        "flow-run-back",
                        if run.execution.content
                            == super::flows::execution_detail::ExecutionContent::Report
                        {
                            s::flow::run_return_to_execution()
                        } else if run.return_to.is_some() {
                            s::flow::back_to_definition()
                        } else {
                            s::flow::back_to_list()
                        },
                    ),
                };
                (
                    self.project_for(detail.lane.project)?.name.clone(),
                    item,
                    Some(detail.lane),
                    back_id,
                    back_label,
                )
            }
            Page::PullRequests => {
                let detail = self.pages.pull_requests.detail.as_ref()?;
                (
                    detail.item.repo.qualified(),
                    format!("#{}", detail.item.pr.number),
                    self.pr_detail_lane(&detail.item).map(|lane| lane.lane),
                    "pr-detail-back",
                    s::pull_requests::back_to_list(),
                )
            }
        };
        Some(Location {
            owner,
            item,
            lane,
            back_id,
            back_label,
        })
    }
}

pub(super) fn render(ws: &Workspace, page: Page, cx: &Context<Workspace>) -> Option<AnyElement> {
    let location = ws.detail_location(page, cx)?;
    let workspace = cx.weak_entity();
    let id = location.back_id;
    Some(
        div()
            .flex()
            .items_center()
            .min_w_0()
            .flex_none()
            .gap(px(theme::GAP_SM))
            .px(px(theme::PAD_LG))
            .py(px(theme::PAD_XS))
            .border_b_1()
            .border_color(theme::current(cx).border)
            .text_size(px(theme::FONT_SIZE_SM))
            .text_color(theme::current(cx).text_muted)
            .child(
                button_icon(id, icons::BACK, cx)
                    .tab_stop(true)
                    .tooltip(location.back_label)
                    .debug_selector(move || id.into())
                    .on_click(cx.listener(|ws, _, window, cx| {
                        ws.back_on_page(window, cx);
                    })),
            )
            .child(div().min_w_0().truncate().child(location.owner))
            .child(icons::icon(icons::CHEVRON_RIGHT))
            .child(div().flex_1().min_w_0().truncate().child(location.item))
            .when(location.lane.is_none(), |row| {
                row.child(s::common::worktree_unassigned())
            })
            .children(location.lane.map(|lane| {
                super::chrome::worktree_button(
                    "page-detail-worktree",
                    lane,
                    ws.lane_label_for(lane),
                    workspace,
                )
            }))
            .when(page == Page::PullRequests, |row| {
                row.child(
                    button_icon("pr-detail-refresh", icons::REFRESH, cx)
                        .tooltip(s::common::refresh())
                        .on_click(cx.listener(|ws, _, _, cx| ws.refresh_shown_pr_detail(cx))),
                )
            })
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Ownership and retained scope are covered by pages::scope::tests.
}
