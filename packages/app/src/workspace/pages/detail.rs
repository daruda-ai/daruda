//! What a page shows over its list — its detail — and the one way to leave
//! one. A detail outlives the page being hidden; only leaving it clears it,
//! and leaving asks first when it holds unsaved edits.

use gpui::{App, Context, Window};

use super::Page;
use super::flows::detail::{FlowDetailId, FlowsPage};
use super::pull_requests::PullRequestsPage;
use super::pull_requests::detail::PrDetailId;
use super::tasks::{TaskEditorId, TasksPage};
use crate::surface::strings;
use crate::workspace::Workspace;
use crate::workspace::dirty_items::DirtyItem;

/// What each page remembers while it is not on screen.
pub(in crate::workspace) struct PageDetails {
    pub(super) scope: super::scope::ProjectScope,
    pub(super) last_page: Option<Page>,
    pub(super) scrolls: [gpui::ScrollHandle; 3],
    pub tasks: TasksPage,
    pub flows: FlowsPage,
    pub pull_requests: PullRequestsPage,
}

impl PageDetails {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            scope: Default::default(),
            last_page: None,
            scrolls: std::array::from_fn(|_| gpui::ScrollHandle::new()),
            tasks: Default::default(),
            flows: Default::default(),
            pull_requests: PullRequestsPage::new(window, cx),
        }
    }
}

/// Names the detail a page shows. Ids are never reused, so a callback that
/// arrives after its detail was replaced finds nothing to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum PageDetailId {
    /// A detail that can hold unsaved edits, which leaving it asks about.
    Editable(EditableDetailId),
    PullRequest(PrDetailId),
}

/// The details that can hold unsaved edits. Only these are dirty-checked
/// and saved, so a read-only detail needs no arm there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum EditableDetailId {
    TaskEditor(TaskEditorId),
    Flow(FlowDetailId),
}

impl PageDetailId {
    fn page(self) -> Page {
        match self {
            Self::Editable(EditableDetailId::TaskEditor(_)) => Page::Tasks,
            Self::Editable(EditableDetailId::Flow(_)) => Page::Flows,
            Self::PullRequest(_) => Page::PullRequests,
        }
    }
}

impl From<TaskEditorId> for PageDetailId {
    fn from(id: TaskEditorId) -> Self {
        Self::Editable(EditableDetailId::TaskEditor(id))
    }
}

impl From<FlowDetailId> for PageDetailId {
    fn from(id: FlowDetailId) -> Self {
        Self::Editable(EditableDetailId::Flow(id))
    }
}

impl From<PrDetailId> for PageDetailId {
    fn from(id: PrDetailId) -> Self {
        Self::PullRequest(id)
    }
}

impl Workspace {
    /// A retained detail is drawn only in its own browsing context. Hidden
    /// pages may keep unsaved work while another page changes the scope.
    pub(in crate::workspace) fn shown_page_detail_id(&self) -> Option<PageDetailId> {
        let page = self.active_page()?;
        let matches = match page {
            Page::Tasks => self.pages.tasks.detail.as_ref().is_some_and(|detail| {
                detail.project.is_none_or(|project| {
                    self.page_scope()
                        .matches(project, self.active_project().map(|project| project.uuid))
                })
            }),
            Page::Flows => self
                .pages
                .flows
                .detail
                .as_ref()
                .is_some_and(|detail| self.page_scope_matches(detail.lane.project)),
            Page::PullRequests => {
                !self.pages.pull_requests.list_covers_detail()
                    && self.pages.pull_requests.detail().is_some_and(|detail| {
                        self.page_scope() == super::scope::ProjectScope::All
                            || self.github_repos().contains(&detail.item().repo)
                    })
            }
        };
        matches.then(|| self.page_detail_id(page)).flatten()
    }

    /// The detail `page` shows over its list, if any.
    pub(in crate::workspace) fn page_detail_id(&self, page: Page) -> Option<PageDetailId> {
        match page {
            Page::Tasks => self.task_detail_id().map(PageDetailId::from),
            Page::Flows => self.flow_detail_id().map(PageDetailId::from),
            Page::PullRequests => self.pr_detail_id().map(PageDetailId::from),
        }
    }

    /// Detail `id` as unsaved work, or `None` when it holds none.
    fn page_detail_dirty(&self, id: PageDetailId, cx: &App) -> Option<DirtyItem> {
        let PageDetailId::Editable(id) = id else {
            return None;
        };
        match id {
            EditableDetailId::TaskEditor(editor) => {
                DirtyItem::of_task_editor(editor, self.task_editor(editor)?, cx)
            }
            EditableDetailId::Flow(detail) => {
                if self.flow_detail_id() != Some(detail) {
                    return None;
                }
                let (graph, _, view) = self.open_graph()?;
                DirtyItem::of_flow_graph(graph, &view, cx)
            }
        }
    }

    /// Leave `page`'s detail, then run `next`. Unsaved edits ask first —
    /// Save goes on only if the save landed (a failed one says so and stays),
    /// Discard drops them, Cancel stays. Back, Escape, closing and opening another detail all come here.
    pub(in crate::workspace) fn leave_page_detail_then(
        &mut self,
        page: Page,
        window: &mut Window,
        cx: &mut Context<Self>,
        next: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(id) = self.page_detail_id(page) else {
            next(self, window, cx);
            return;
        };
        let Some(item) = self.page_detail_dirty(id, cx) else {
            self.leave_detail(id, window, cx);
            next(self, window, cx);
            return;
        };
        let save = item.save_label();
        let discard = strings::task::edit_discard();
        let cancel = strings::common::btn_cancel();
        let receiver = window.prompt(
            gpui::PromptLevel::Warning,
            &item.leave_heading(),
            None,
            &[save.as_str(), discard.as_str(), cancel.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(answer) = receiver.await else {
                return;
            };
            let saved = if answer == 0 {
                let Ok(Some(pending)) = this.update_in(cx, |workspace, window, cx| {
                    (workspace.page_detail_id(page) == Some(id)).then(|| {
                        workspace.commit_dirty_items_with_failure_toast(
                            std::slice::from_ref(&item),
                            window,
                            cx,
                        )
                    })
                }) else {
                    return;
                };
                pending.await
            } else {
                answer == 1
            };
            // SILENT-OK: the window may close before the answer arrives
            let _ = this.update_in(cx, |ws, window, cx| {
                // Answered for a detail since replaced: nothing to leave.
                if ws.page_detail_id(page) != Some(id) {
                    return;
                }
                if saved {
                    ws.leave_detail(id, window, cx);
                    next(ws, window, cx);
                }
            });
        })
        .detach();
    }

    /// Leave the detail of the page on screen, asking first if it holds
    /// edits. `false` when no page is shown or it shows only its list, so
    /// the caller closes what it would.
    pub(in crate::workspace) fn close_page_detail(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(page) = self.active_page() else {
            return false;
        };
        if self.shown_page_detail_id().is_none() {
            return false;
        }
        self.leave_page_detail_then(page, window, cx, |_, _, _| {});
        true
    }

    /// Escape follows the same Back path, including execution subviews and
    /// the definition that opened one. A local rename may consume it first.
    pub(in crate::workspace) fn on_page_key_down(
        &mut self,
        ev: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.keystroke.key != "escape" {
            return;
        }
        if self.leave_pr_search(window, cx) || self.back_on_page(window, cx) {
            cx.stop_propagation();
        }
    }

    pub(in crate::workspace) fn back_on_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_page() == Some(Page::Flows)
            && self.shown_page_detail_id().is_some()
            && self.pages.flows.detail.as_ref().is_some_and(|detail| {
                matches!(detail.body, super::flows::detail::FlowDetailBody::Run(_))
            })
        {
            self.back_from_execution(window, cx);
            true
        } else {
            self.close_page_detail(window, cx)
        }
    }

    /// The project or lane set changed: a detail naming what is gone drops,
    /// and the Task editor's pickers refill. Called wherever a project opens
    /// or closes, a lane is created or removed, or discovery replaces a
    /// project's placeholder lanes.
    pub(in crate::workspace) fn page_details_follow_projects(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_flow_detail_of_gone_lane(window, cx);
        self.refresh_task_worktrees(cx);
        self.refresh_task_editor_choices(window, cx);
        if let super::scope::ProjectScope::Project(project) = self.page_scope()
            && self.project_by_uuid(project).is_none()
        {
            let scope = self
                .active_project()
                .map_or(super::scope::ProjectScope::All, |p| {
                    super::scope::ProjectScope::Project(p.uuid)
                });
            self.set_page_scope(scope, cx);
        }
    }

    /// Drop detail `id` and give the keyboard back to the page: the focus
    /// was inside what is being dropped, and nothing else would take it.
    pub(in crate::workspace) fn leave_detail(
        &mut self,
        id: PageDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_page_detail_now(id, cx);
        self.focus_shown_page(window, cx);
    }

    /// Drop detail `id` because what it shows is gone. The keyboard comes back
    /// to the page only if the page is on screen; otherwise focus stays where
    /// the person is.
    pub(in crate::workspace) fn drop_gone_detail(
        &mut self,
        id: PageDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_page() == Some(id.page()) {
            self.leave_detail(id, window, cx);
        } else {
            self.close_page_detail_now(id, cx);
        }
    }

    /// Drop detail `id` without asking. A stale id drops nothing.
    pub(in crate::workspace) fn close_page_detail_now(
        &mut self,
        id: PageDetailId,
        cx: &mut Context<Self>,
    ) {
        if self.page_detail_id(id.page()) != Some(id) {
            return;
        }
        self.mutate_durable(cx, |ws, _| match id.page() {
            Page::Tasks => ws.pages.tasks.detail = None,
            Page::Flows => ws.pages.flows.detail = None,
            Page::PullRequests => ws.pages.pull_requests.replace_detail(None),
        });
        // What was typed in a pull request's detail is on disk before it is
        // gone from view.
        if id.page() == Page::PullRequests {
            self.settle_pr_details(cx);
            crate::github::DraftStore::flush(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_detail_id_names_its_page_and_only_edits_are_editable() {
        // (detail, its page, editable)
        let cases = [
            (PageDetailId::from(TaskEditorId(1)), Page::Tasks, true),
            (PageDetailId::from(FlowDetailId(1)), Page::Flows, true),
            (PageDetailId::from(PrDetailId(1)), Page::PullRequests, false),
        ];
        for (id, page, editable) in cases {
            assert_eq!(id.page(), page, "{id:?}");
            assert_eq!(matches!(id, PageDetailId::Editable(_)), editable, "{id:?}");
        }
    }
}
