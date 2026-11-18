//! The Pull requests page: the open pull requests of every GitHub repository
//! this window's worktrees push to, grouped by repository. The list itself
//! is kept by [`crate::workspace::github_ops::pr_list`], searched only while
//! the page is shown (its hooks in `pages/shown.rs`); this page stages what
//! it draws.

use crate::ui::InputState;
use daruda_github::{PrFilter, PrSearchState};
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
pub(in crate::workspace) use lane::CheckoutStage;
pub(in crate::workspace) mod list;
pub(in crate::workspace) mod navigation;
pub(in crate::workspace) mod page_layout;
mod progress;
mod remote_search;
pub(in crate::workspace) mod render;
pub(in crate::workspace) mod row_layout;
mod search;
mod split;

use std::collections::BTreeSet;

use detail::PrDetailState;
use detail::view::PrDetailView;
use list::{LaneFacts, PageInputs, PrKey, PrPageView};

/// What the page remembers while it is not on screen. Never persisted.
pub(in crate::workspace) struct PullRequestsPage {
    /// The filter the page shows, and shows again when reopened. The one
    /// place the selected tab lives; the cache holds every tab's answers.
    filter: PrFilter,
    /// Which pull requests the tabs list: open (drafts too), merged,
    /// closed unmerged, or all. Shown again when the page reopens.
    state: PrSearchState,
    /// What GitHub answered for every tab while the page is on screen.
    pub(in crate::workspace) cache: PrListCache,
    search: Entity<InputState>,
    /// Pull requests a worktree is being made for, so their rows do not
    /// offer to make a second one.
    creating: BTreeSet<PrKey>,
    /// How far each worktree being made has got.
    checkout_stages: std::collections::HashMap<PrKey, lane::CheckoutStage>,
    /// Staged each frame while the page shows its list; `None` otherwise.
    pub(in crate::workspace) snapshot: Option<PrPageSnapshot>,
    /// The pull request shown over the list, if any.
    pub(in crate::workspace) detail: Option<PrDetailState>,
    /// Staged each frame while the page shows its detail; `None` otherwise.
    pub(in crate::workspace) detail_snapshot: Option<PrDetailView>,
    /// Files marked viewed and where each detail closed, while the window
    /// is open.
    pub(in crate::workspace) progress: detail::progress::ReviewProgress,
    /// Takes the keyboard while the list is shown, so the arrows and Enter
    /// browse it; the search field sits outside it.
    focus: FocusHandle,
    /// The selected row: moved by the arrows, set by opening a detail, and
    /// where the list comes back to. Kept while search hides it.
    cursor: Option<PrKey>,
    row_bounds: navigation::RowBounds,
    /// The width between the docks at the last frame; measured before
    /// staging.
    content_width: f32,
    /// The list column's width beside a detail, as last chosen; the window
    /// may narrow what is drawn without changing it.
    list_width: f32,
    list_column: page_layout::ListColumn,
    divider_drag: Option<split::DividerDrag>,
    /// What the last staged frame drew; a change re-flows the rows.
    staged_layout: Option<page_layout::PageLayout>,
    /// Where the list stood in the layout last left, to come back to.
    return_point: Option<ReturnPoint>,
    /// The list was laid out anew this frame: once it is drawn, bring the
    /// selected row back into view.
    reveal_pending: bool,
    /// Whether the search narrows the loaded rows or asks GitHub.
    search_mode: remote_search::SearchMode,
    /// The GitHub search last sent: the field's text once typing paused.
    committed: crate::github::Refine,
    /// Sends the GitHub search once typing pauses; replaced on each edit.
    debounce: Option<gpui::Task<()>>,
    _search_changes: gpui::Subscription,
}

impl PullRequestsPage {
    pub(in crate::workspace) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The width between the docks this frame, before the page is staged.
    pub(in crate::workspace) fn measure(&mut self, content: f32) {
        self.content_width = content;
    }

    /// Whether the window holds the list beside a detail.
    pub(in crate::workspace) fn split_fits(&self) -> bool {
        page_layout::PageLayout::split_fits(self.content_width)
    }

    /// Whether this frame laid the list out anew; asking clears it.
    pub(in crate::workspace) fn take_reveal_pending(&mut self) -> bool {
        std::mem::take(&mut self.reveal_pending)
    }

    /// Show `next` as the detail, or none; the one it replaces leaves its
    /// place in the review progress to come back to.
    pub(in crate::workspace) fn replace_detail(&mut self, next: Option<PrDetailState>) {
        if let Some(gone) = std::mem::replace(&mut self.detail, next) {
            self.progress.leave(&gone.key(), gone.place());
        }
    }

    /// The list column's width beside a detail this frame.
    pub(in crate::workspace) fn list_width(&self) -> f32 {
        page_layout::list_width(self.content_width, self.list_width)
    }

    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::surface::strings::pull_requests::search_placeholder())
        });
        let search_changes = cx.subscribe(&search, |ws, _, event: &crate::ui::InputEvent, cx| {
            if matches!(event, crate::ui::InputEvent::Change) {
                ws.pr_search_edited(cx);
            }
        });
        Self {
            filter: PrFilter::Mine,
            state: PrSearchState::Open,
            search,
            creating: BTreeSet::new(),
            checkout_stages: std::collections::HashMap::new(),
            cache: PrListCache::default(),
            snapshot: None,
            detail: None,
            detail_snapshot: None,
            progress: detail::progress::ReviewProgress::new(),
            focus: cx.focus_handle(),
            cursor: None,
            row_bounds: navigation::RowBounds::default(),
            content_width: 0.0,
            list_width: crate::ui::theme::PR_SPLIT_LIST_W,
            list_column: page_layout::ListColumn::Beside,
            divider_drag: None,
            staged_layout: None,
            return_point: None,
            reveal_pending: false,
            search_mode: remote_search::SearchMode::Loaded,
            committed: crate::github::Refine::default(),
            debounce: None,
            _search_changes: search_changes,
        }
    }
}

/// The list's scroll in a layout it left, with the row selected then.
#[derive(Debug, Clone, PartialEq)]
struct ReturnPoint {
    layout: page_layout::PageLayout,
    offset: gpui::Point<gpui::Pixels>,
    cursor: Option<PrKey>,
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
    pub mode: remote_search::SearchMode,
    /// The field holds text, whether or not a GitHub search has sent it.
    pub typed: bool,
    /// The GitHub search's facets, shown as removable chips.
    pub facets: Vec<crate::github::Facet>,
    /// Why the typed GitHub search was not sent with the shown tab.
    pub problem: Option<crate::github::RefineProblem>,
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

    /// Which pull requests the tabs list.
    pub(in crate::workspace) fn pr_page_state(&self) -> PrSearchState {
        self.pages.pull_requests.state
    }

    /// List `state`'s pull requests on every tab. Each state is its own
    /// search; the first time one is shown it is asked at once.
    pub(in crate::workspace) fn set_pr_page_state(
        &mut self,
        state: PrSearchState,
        cx: &mut Context<Self>,
    ) {
        if self.pages.pull_requests.state == state {
            return;
        }
        self.mutate_durable(cx, |ws, _| ws.pages.pull_requests.state = state);
        self.prune_pr_list();
        PrList::wanted(cx);
        cx.notify();
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
        self.mutate_durable(cx, |ws, _| ws.pages.pull_requests.filter = filter);
        self.prune_pr_list();
        // A GitHub search is sent for the shown tab alone.
        if !self.pr_page_refine().is_empty() {
            PrList::wanted(cx);
        }
        cx.notify();
    }

    /// What the shown page draws: the list, its detail, or both.
    pub(in crate::workspace) fn pr_page_layout(&self) -> page_layout::PageLayout {
        let page = &self.pages.pull_requests;
        page_layout::PageLayout::of(
            page.detail.is_some() && self.shown_page_detail_id().is_some(),
            page.split_fits() && page.list_column == page_layout::ListColumn::Beside,
        )
    }

    /// Stage what the page draws this frame: its detail when it holds one,
    /// its list when no detail covers it. Only while it is on screen: the
    /// rows cost a pass over every lane and pull request, so the lanes and
    /// repositories are read once here for everything the frame draws.
    pub(in crate::workspace) fn stage_pr_page(&mut self, cx: &Context<Self>) {
        if self.active_page() != Some(Page::PullRequests) {
            self.pages.pull_requests.detail_snapshot = None;
            self.pages.pull_requests.snapshot = None;
            self.pages.pull_requests.staged_layout = None;
            return;
        }
        let layout = self.pr_page_layout();
        self.follow_pr_layout(layout);
        let lanes = self.pr_lanes();
        let detail = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|_| self.shown_page_detail_id().is_some())
            .map(|state| {
                let lane = list::linked_lane(state.item(), &lanes);
                let draft_owner = crate::github::DraftStore::owner(
                    &state.item().repo,
                    state.item().pr.number,
                    cx,
                );
                let inputs = detail::view::DetailInputs {
                    lane,
                    link: self.pr_link_inputs(state.item(), lane, &lanes, cx),
                    sending: lane.is_some_and(|lane| self.is_sending_pr(lane.lane)),
                    creating: &self.pages.pull_requests.creating,
                    checkout_stage: self
                        .pages
                        .pull_requests
                        .checkout_stages
                        .get(&(state.item().repo.clone(), state.item().pr.number))
                        .copied(),
                    progress: &self.pages.pull_requests.progress,
                    drafts: draft_owner
                        .as_ref()
                        .and_then(|owner| crate::github::DraftStore::of(owner, cx)),
                    keeping: crate::github::DraftStore::keeping(draft_owner.as_ref(), cx),
                    handoff_chats: state
                        .handoff()
                        .map(|handoff| self.writable_chats(handoff.lane, cx))
                        .unwrap_or_default(),
                    now: chrono::Utc::now(),
                };
                detail::view::view(state, &inputs)
            });
        let shows_list = layout.shows_list();
        self.pages.pull_requests.detail_snapshot = detail;
        self.pages.pull_requests.row_bounds.borrow_mut().clear();
        self.pages.pull_requests.snapshot = shows_list.then(|| {
            let repos = self.github_repos();
            let search = self.pages.pull_requests.search.clone();
            let typed = !search.read(cx).value().trim().is_empty();
            let mut list = self.pr_page_view(&repos, &lanes, cx);
            let (query, facets, problem) = match self.pr_page_search(cx) {
                remote_search::PageSearch::Loaded(query) => (query, Vec::new(), None),
                remote_search::PageSearch::GitHub(refine) => {
                    (refine.text(), refine.facets.clone(), None)
                }
                remote_search::PageSearch::Blocked(problem) => {
                    // Nothing was asked for it: neither the bare tab's rows,
                    // its later pages, nor its age and sources stand for it.
                    list.body = list::PageBody::Groups(Vec::new());
                    list.more = list::More::None;
                    list.found = None;
                    list.refreshed = None;
                    list.sources = progress::Sources::default();
                    let committed = &self.pages.pull_requests.committed;
                    (committed.text(), committed.facets.clone(), Some(problem))
                }
            };
            let (visible, total) = match self.pr_search_mode() {
                remote_search::SearchMode::Loaded => search::narrow(&mut list, &query),
                remote_search::SearchMode::GitHub => {
                    let shown = navigation::rows(&list).count();
                    (shown, shown)
                }
            };
            let counts = if repos.is_empty() && list.activity.busy() {
                std::array::from_fn(|_| crate::surface::strings::pull_requests::count_pending())
            } else {
                counts::labels(self.pr_list(), self.pr_page_state(), &repos, |host| {
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
                mode: self.pr_search_mode(),
                typed,
                facets,
                problem,
            }
        });
    }

    /// Rows change shape between the table, the column beside a detail, and
    /// not being drawn, so one scroll offset means different rows in each.
    /// Coming back to the layout last left with the same row selected
    /// returns to its exact place; any other change brings the selected row
    /// into view once the new layout is painted.
    fn follow_pr_layout(&mut self, layout: page_layout::PageLayout) {
        let scroll = &self.pages.scrolls[Page::PullRequests.index()];
        let page = &mut self.pages.pull_requests;
        let last = page.staged_layout.replace(layout);
        let Some(last) = last.filter(|last| *last != layout) else {
            return;
        };
        let previous = page.return_point.take();
        if last.shows_list() {
            page.return_point = Some(ReturnPoint {
                layout: last,
                offset: scroll.offset(),
                cursor: page.cursor.clone(),
            });
        }
        if !layout.shows_list() {
            return;
        }
        match previous.filter(|point| point.layout == layout && point.cursor == page.cursor) {
            Some(point) => scroll.set_offset(point.offset),
            None => page.reveal_pending = true,
        }
    }

    pub(in crate::workspace) fn clear_pr_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Silently: the search is applied here, at once, not after a pause.
        self.pages
            .pull_requests
            .search
            .update(cx, |input, cx| input.set_value_silently("", window, cx));
        self.commit_pr_search(cx);
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
            refreshing: PrList::is_refreshing(
                self.pr_page_filter(),
                self.pr_page_state(),
                self.pr_page_refine(),
                repos,
                cx,
            ),
            loading_more: PrList::is_loading_more(
                self.pr_page_filter(),
                self.pr_page_state(),
                self.pr_page_refine(),
                repos,
                cx,
            ),
            filter: self.pr_page_filter(),
            state: self.pr_page_state(),
            refine: self.pr_page_refine(),
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
