//! The Pull requests page: the open pull requests of every GitHub repository
//! this window's worktrees push to, grouped by repository. The list itself
//! is kept by [`crate::workspace::github_ops::pr_list`], searched only while
//! the page is shown (its hooks in `pages/shown.rs`); this page stages what
//! it draws.

use crate::ui::InputState;
use daruda_github::PrFilter;
use daruda_store::project::LaneKind;
use gpui::{App, AppContext as _, Context, Entity, FocusHandle, WeakEntity, Window};

use super::Page;
use crate::github::{PrList, PrPoller};
use crate::workspace::Workspace;
use crate::workspace::github_ops::pr_list::PrListCache;
use crate::workspace::lane_scoped::LaneScoped;

pub(in crate::workspace) mod actions;
mod controls;
mod counts;
pub(in crate::workspace) mod detail;
mod lane;
pub(in crate::workspace) mod list;
pub(in crate::workspace) mod navigation;
mod progress;
pub(in crate::workspace) mod render;
pub(in crate::workspace) mod row_layout;
mod search;

use std::collections::BTreeSet;

use detail::PrDetailState;
use detail::view::PrDetailView;
use list::{LaneFacts, PageInputs, PrKey, PrPageView};

/// What the page remembers while it is not on screen. Never persisted.
pub(in crate::workspace) struct PullRequestsPage {
    /// The filter the page shows, and shows again when reopened. The one
    /// place the selected tab lives; the cache holds every tab's answers.
    filter: PrFilter,
    /// What GitHub answered for every tab while the page is on screen.
    pub(in crate::workspace) cache: PrListCache,
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
    /// Takes the keyboard while the list is shown, so the arrows and Enter
    /// browse it; the search field sits outside it.
    focus: FocusHandle,
    /// The selected row: moved by the arrows, set by opening a detail, and
    /// where the list comes back to. Kept while search hides it.
    cursor: Option<PrKey>,
    row_bounds: navigation::RowBounds,
}

impl PullRequestsPage {
    pub(in crate::workspace) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        Self {
            filter: PrFilter::Mine,
            search: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(crate::surface::strings::pull_requests::search_placeholder())
            }),
            creating: BTreeSet::new(),
            cache: PrListCache::default(),
            snapshot: None,
            detail: None,
            detail_snapshot: None,
            focus: cx.focus_handle(),
            cursor: None,
            row_bounds: navigation::RowBounds::default(),
        }
    }
}

/// Plain data for one frame of the page, so drawing it reads no entity.
pub(in crate::workspace) struct PrPageSnapshot {
    pub workspace: WeakEntity<Workspace>,
    pub list: PrPageView,
    pub scope: super::scope::ScopeSnapshot,
    pub search: Entity<InputState>,
    /// The search as typed, trimmed: empty means no search.
    pub query: String,
    pub visible: usize,
    pub total: usize,
    pub counts: [String; 3],
    pub focus: FocusHandle,
    pub cursor: Option<PrKey>,
    pub row_bounds: navigation::RowBounds,
}

impl Workspace {
    /// The page came on screen: search for its list until it leaves.
    pub(super) fn pr_page_shown(&mut self, cx: &mut Context<Self>) {
        self.show_pr_list(cx);
        self.refresh_pr_repositories(cx);
    }

    /// The tab the page shows, and shows again when reopened.
    pub(in crate::workspace) fn pr_page_filter(&self) -> PrFilter {
        self.pages.pull_requests.filter
    }

    /// Show `filter`'s pull requests, now and whenever the page reopens.
    /// Every tab's answers are already held, so switching asks nothing.
    pub(in crate::workspace) fn set_pr_page_filter(
        &mut self,
        filter: PrFilter,
        cx: &mut Context<Self>,
    ) {
        if self.pages.pull_requests.filter == filter {
            return;
        }
        self.pages.pull_requests.filter = filter;
        cx.notify();
    }

    /// Stage what the page draws this frame: its detail when it holds one,
    /// its list otherwise. Only while it is on screen: the rows cost a pass
    /// over every lane and pull request, so the lanes and repositories are
    /// read once here for everything the frame draws.
    pub(in crate::workspace) fn stage_pr_page(&mut self, cx: &Context<Self>) {
        if self.active_page() != Some(Page::PullRequests) {
            self.pages.pull_requests.detail_snapshot = None;
            self.pages.pull_requests.snapshot = None;
            return;
        }
        let lanes = self.pr_lanes();
        let detail = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|_| self.shown_page_detail_id().is_some())
            .map(|state| {
                let lane = list::linked_lane(state.item(), &lanes);
                let inputs = detail::view::DetailInputs {
                    lane,
                    sending: lane.is_some_and(|lane| self.is_sending_pr(lane.lane)),
                    creating: &self.pages.pull_requests.creating,
                    now: chrono::Utc::now(),
                };
                detail::view::view(state, &inputs)
            });
        let shows_list = detail.is_none();
        self.pages.pull_requests.detail_snapshot = detail;
        self.pages.pull_requests.row_bounds.borrow_mut().clear();
        self.pages.pull_requests.snapshot = shows_list.then(|| {
            let repos = self.github_repos();
            let search = self.pages.pull_requests.search.clone();
            let query = search.read(cx).value().trim().to_owned();
            let mut list = self.pr_page_view(&repos, &lanes, cx);
            let (visible, total) = search::narrow(&mut list, &query);
            let counts = if repos.is_empty() && list.activity.busy() {
                std::array::from_fn(|_| crate::surface::strings::pull_requests::count_pending())
            } else {
                counts::labels(self.pr_list(), &repos, |host| {
                    PrPoller::availability(host, cx)
                })
            };
            PrPageSnapshot {
                workspace: cx.weak_entity(),
                scope: self.scope_snapshot(),
                list,
                search,
                query,
                visible,
                total,
                counts,
                focus: self.pages.pull_requests.focus.clone(),
                cursor: self.pages.pull_requests.cursor.clone(),
                row_bounds: self.pages.pull_requests.row_bounds.clone(),
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

    /// The page's list as it stands, for a test outside a frame.
    #[cfg(test)]
    pub(in crate::workspace) fn pr_page_list(&self, cx: &App) -> PrPageView {
        self.pr_page_view(&self.github_repos(), &self.pr_lanes(), cx)
    }

    fn pr_page_view(
        &self,
        repos: &BTreeSet<daruda_github::RepoId>,
        lanes: &[LaneFacts],
        cx: &App,
    ) -> PrPageView {
        let inputs = PageInputs {
            enabled: crate::github::enabled(cx),
            discovering: lanes.iter().any(|lane| {
                self.page_scope_matches(lane.lane.project)
                    && self.lane_scoped.get(&lane.lane).is_some_and(|state| {
                        use crate::workspace::lane_scoped::RefreshSlot;
                        matches!(state.project_discovery, RefreshSlot::Running { .. })
                            || matches!(state.git.push_target_refresh, RefreshSlot::Running { .. })
                    })
            }),
            refreshing: PrList::is_refreshing(self.pr_page_filter(), repos, cx),
            filter: self.pr_page_filter(),
            repos,
            list: self.pr_list(),
            lanes,
            creating: &self.pages.pull_requests.creating,
            now: chrono::Utc::now(),
            clock: cx.background_executor().now(),
            accounts: &repos
                .iter()
                .filter_map(|repo| {
                    let account = PrPoller::account(repo.host(), cx)?;
                    Some((repo.host().to_owned(), account))
                })
                .collect(),
        };
        list::page(&inputs, |host| PrPoller::availability(host, cx))
    }

    /// Every lane in the window, with what matching it to a pull request
    /// reads. Not narrowed to the browsed scope: which worktree works on a
    /// pull request does not change with what the page shows. A lane the
    /// scope does not single out names its project.
    pub(super) fn pr_lanes(&self) -> Vec<LaneFacts> {
        let all = self.page_scope() == super::scope::ProjectScope::All;
        self.projects
            .lanes()
            .map(|(lane, project, record)| LaneFacts {
                lane,
                name: if all || !self.page_scope_matches(lane.project) {
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
