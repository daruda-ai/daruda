//! The Pull requests page: the open pull requests of every GitHub repository
//! this window's worktrees push to, grouped by repository. The list itself
//! is kept by [`crate::workspace::github_ops::pr_list`], searched only while
//! the page is shown (its hooks in `pages/shown.rs`); this page stages what
//! it draws.

use crate::ui::InputState;
use daruda_github::PrFilter;
use daruda_store::project::LaneKind;
use gpui::{App, AppContext as _, Context, Entity, WeakEntity, Window};

use super::Page;
use crate::github::PrPoller;
use crate::workspace::Workspace;
use crate::workspace::lane_scoped::LaneScoped;

pub(in crate::workspace) mod actions;
mod controls;
mod counts;
pub(in crate::workspace) mod detail;
mod lane;
pub(in crate::workspace) mod list;
pub(in crate::workspace) mod render;
mod search;

use std::collections::BTreeSet;

use detail::PrDetailState;
use detail::view::PrDetailView;
use list::{LaneFacts, PageInputs, PrKey, PrPageList};

/// What the page remembers while it is not on screen. Never persisted.
pub(in crate::workspace) struct PullRequestsPage {
    /// The filter the page shows, and shows again when reopened.
    filter: PrFilter,
    search: Entity<InputState>,
    /// Pull requests a worktree is being made for, so their rows do not
    /// offer to make a second one.
    creating: BTreeSet<PrKey>,
    /// Staged each frame while the page shows its list; `None` otherwise.
    pub(in crate::workspace) snapshot: Option<PrPageSnapshot>,
    /// The pull request shown over the list, if any.
    pub(in crate::workspace) detail: Option<PrDetailState>,
    /// Staged each frame while the page shows its detail; `None` otherwise.
    pub(in crate::workspace) detail_snapshot: Option<PrDetailView>,
}

impl PullRequestsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            filter: PrFilter::Mine,
            search: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::surface::strings::pull_requests::search_placeholder())
            }),
            creating: BTreeSet::new(),
            snapshot: None,
            detail: None,
            detail_snapshot: None,
        }
    }
}

/// Plain data for one frame of the page, so drawing it reads no entity.
pub(in crate::workspace) struct PrPageSnapshot {
    pub workspace: WeakEntity<Workspace>,
    pub list: PrPageList,
    pub scope: super::scope::ScopeSnapshot,
    pub search: Entity<InputState>,
    pub query: String,
    pub visible: usize,
    pub total: usize,
    pub counts: [String; 3],
}

impl Workspace {
    /// The page came on screen: search for its list until it leaves.
    pub(super) fn pr_page_shown(&mut self, cx: &mut Context<Self>) {
        self.show_pr_list(self.pages.pull_requests.filter, cx);
        self.refresh_pr_repositories(cx);
    }

    /// Show `filter`'s pull requests, now and whenever the page reopens.
    pub(in crate::workspace) fn set_pr_page_filter(
        &mut self,
        filter: PrFilter,
        cx: &mut Context<Self>,
    ) {
        if self.pages.pull_requests.filter == filter {
            return;
        }
        self.pages.pull_requests.filter = filter;
        if self.active_page() == Some(Page::PullRequests) {
            self.show_pr_list(filter, cx);
        }
        cx.notify();
    }

    /// Stage what the page draws this frame: its detail when it holds one,
    /// its list otherwise. Only while it is on screen: the rows cost a pass
    /// over every lane and pull request.
    pub(in crate::workspace) fn stage_pr_page(&mut self, cx: &Context<Self>) {
        let shown = self.active_page() == Some(Page::PullRequests);
        let detail = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|_| shown && self.shown_page_detail_id().is_some())
            .map(|state| {
                let lane = self.pr_detail_lane(&state.item);
                let inputs = detail::view::DetailInputs {
                    lane: lane.as_ref(),
                    sending: lane
                        .as_ref()
                        .is_some_and(|lane| self.is_sending_pr(lane.lane)),
                    creating: &self.pages.pull_requests.creating,
                    now: chrono::Utc::now(),
                };
                detail::view::view(state, &inputs)
            });
        let list = shown && detail.is_none();
        self.pages.pull_requests.detail_snapshot = detail;
        self.pages.pull_requests.snapshot = list.then(|| {
            let search = self.pages.pull_requests.search.clone();
            let query = search.read(cx).value().to_string();
            let mut list = self.pr_page_list(cx);
            let (visible, total) = search::narrow(&mut list, &query);
            PrPageSnapshot {
                workspace: cx.weak_entity(),
                scope: self.scope_snapshot(),
                list,
                search,
                query,
                visible,
                total,
                counts: counts::labels(self.pr_list(), &self.github_repos(), |host| {
                    PrPoller::availability(host, cx)
                }),
            }
        });
    }

    pub(in crate::workspace) fn clear_pr_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pages
            .pull_requests
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    pub(in crate::workspace) fn clear_pr_filters(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_pr_search(window, cx);
        self.pages.scrolls[Page::PullRequests.index()]
            .set_offset(gpui::point(gpui::Pixels::ZERO, gpui::Pixels::ZERO));
    }

    pub(in crate::workspace) fn pr_page_list(&self, cx: &App) -> PrPageList {
        let repos = self.github_repos();
        let lanes = self.pr_page_lanes();
        let inputs = PageInputs {
            enabled: crate::github::enabled(cx),
            filter: self.pages.pull_requests.filter,
            repos: &repos,
            list: self.pr_list(),
            lanes: &lanes,
            creating: &self.pages.pull_requests.creating,
            now: chrono::Utc::now(),
            clock: cx.background_executor().now(),
        };
        list::page(&inputs, |host| PrPoller::availability(host, cx))
    }

    /// Every lane, with what matching it to a pull request reads.
    fn pr_page_lanes(&self) -> Vec<LaneFacts> {
        self.projects
            .lanes()
            .filter(|(lane, _, _)| self.page_scope_matches(lane.project))
            .map(|(lane, project, record)| LaneFacts {
                lane,
                name: if self.page_scope() == super::scope::ProjectScope::All {
                    format!("{} / {}", project.name, record.display_name())
                } else {
                    record.display_name()
                },
                branch: match &record.kind {
                    LaneKind::Git { branch, .. } => branch.clone(),
                    LaneKind::Default => None,
                },
                pr: self.lane_scoped.get(&lane).and_then(LaneScoped::pr_key),
            })
            .collect()
    }

    /// How many open pull requests this window's worktrees are on, for the
    /// page's entry in the left dock. Known without a search.
    pub(in crate::workspace) fn open_lane_pr_count(&self) -> usize {
        let prs: std::collections::BTreeSet<_> = self
            .lane_scoped
            .values()
            .filter_map(|state| state.pr.link())
            .filter(|link| link.pr.state.is_active())
            .map(|link| (link.repo.clone(), link.pr.number))
            .collect();
        prs.len()
    }
}
