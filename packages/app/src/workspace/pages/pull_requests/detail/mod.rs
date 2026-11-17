//! The Pull requests page's detail: one pull request's checks, review
//! threads and description, shown over the list. It holds no edits, so
//! leaving it never asks. Reads run on the background executor and land
//! only while the detail that asked is still the one shown.

use std::collections::BTreeMap;

use daruda_github::{ActionsJob, GhError, PrDetail, PrListItem, ReviewThreads};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{App, Context, Window};

use super::actions::{ActionState, PrAction};
use super::list::{self, LaneFacts};

mod read_slot;
pub(in crate::workspace) mod render;
pub(in crate::workspace) mod view;

use read_slot::Tickets;
pub(in crate::workspace) use read_slot::{ReadSlot, ReadTicket};

use crate::gh_host::GhHost;
use crate::github::PrLink;
use crate::surface::strings::error;
use crate::workspace::Workspace;
use crate::workspace::github_ops::review_send::PrSendTarget;
use crate::workspace::pages::Page;
use crate::workspace::pages::detail::PageDetailId;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// Names the page's detail. Never reused, so a read that lands after its
/// detail was replaced or closed finds nothing to fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::workspace) struct PrDetailId(pub u64);

/// Something read from GitHub for the detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum Load<T> {
    Loading,
    Ready(T),
    /// What `gh` said.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::workspace) enum PrTab {
    #[default]
    Checks,
    Threads,
    Description,
}

impl PrTab {
    pub(in crate::workspace) const ALL: [Self; 3] =
        [Self::Checks, Self::Threads, Self::Description];
}

/// A failed Actions job's log, keyed as `(run, job)`.
type JobKey = (u64, u64);

fn job_key(job: ActionsJob) -> JobKey {
    (job.run, job.job)
}

/// The description's link opener. The text is any author's, so a link
/// reaches the system opener only when it is a web page or an e-mail
/// address; any other is swallowed rather than left to the default opener.
pub(in crate::workspace) fn open_description_link(url: &str, _: &mut Window, cx: &mut App) -> bool {
    if daruda_github::model::is_openable_link(url) {
        cx.open_url(url);
    }
    true
}

pub(in crate::workspace) struct PrDetailState {
    id: PrDetailId,
    /// The row it was opened from: what the header shows until the detail
    /// is read, and how its worktree is found.
    item: PrListItem,
    /// The pull request as last read; a refresh keeps it up until the
    /// next read lands.
    pr: ReadSlot<PrDetail>,
    tab: PrTab,
    scroll: gpui::ScrollHandle,
    /// The unresolved threads, read when their tab is first shown.
    threads: Option<ReadSlot<ReviewThreads>>,
    /// The failed Actions jobs whose log is open.
    logs: BTreeMap<JobKey, ReadSlot<String>>,
    /// What it is doing to the pull request on GitHub, or what failed.
    action: ActionState,
    /// Every read this detail sends, whichever slot it fills.
    tickets: Tickets,
}

impl PrDetailState {
    /// A detail for `item`, its pull request not read yet.
    pub(super) fn new(id: PrDetailId, item: PrListItem) -> Self {
        Self {
            id,
            item,
            pr: ReadSlot::settled(Load::Loading),
            tab: PrTab::default(),
            scroll: gpui::ScrollHandle::new(),
            threads: None,
            logs: BTreeMap::new(),
            action: ActionState::Idle,
            tickets: Tickets::default(),
        }
    }

    pub(in crate::workspace) fn id(&self) -> PrDetailId {
        self.id
    }

    pub(in crate::workspace) fn item(&self) -> &PrListItem {
        &self.item
    }

    pub(in crate::workspace) fn scroll(&self) -> &gpui::ScrollHandle {
        &self.scroll
    }

    pub(in crate::workspace) fn load(&self) -> &Load<PrDetail> {
        self.pr.load()
    }

    pub(in crate::workspace) fn tab(&self) -> PrTab {
        self.tab
    }

    /// The pull request as read, when no action is running on it: what an
    /// action may be asked about.
    pub(super) fn idle_read(&self) -> Option<&PrDetail> {
        match (self.action.running(), self.pr.load()) {
            (None, Load::Ready(loaded)) => Some(loaded),
            _ => None,
        }
    }

    pub(super) fn begin_action(&mut self, action: PrAction) {
        self.action = ActionState::Running(action);
    }

    /// `action` came back. `true` when this detail was waiting on it, which
    /// is then idle; a detail running nothing, or something else, is left
    /// as it is.
    pub(super) fn finish_action(&mut self, action: PrAction) -> bool {
        let waiting = self.action == ActionState::Running(action);
        if waiting {
            self.action = ActionState::Idle;
        }
        waiting
    }

    /// Say under the header what the last action's failure was. Called
    /// after the reload the action asks for, which clears an older one.
    pub(super) fn fail_action(&mut self, line: String) {
        self.action = ActionState::Failed(line);
    }

    /// Forget what a reload replaces: the open logs, a failure already
    /// said, and the threads. `Some` with the threads' new read when their
    /// tab is shown, so they are read again at once.
    fn reset_for_refresh(&mut self) -> (ReadTicket, Option<ReadTicket>) {
        self.logs.clear();
        if matches!(self.action, ActionState::Failed(_)) {
            self.action = ActionState::Idle;
        }
        let threads = self.threads.take().is_some() && self.tab == PrTab::Threads;
        (
            self.begin_read(),
            threads.then(|| self.begin_threads_read()),
        )
    }

    /// Send the pull request's next read; what is shown stays until it lands.
    fn begin_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        self.pr.reread(ticket);
        ticket
    }

    fn begin_threads_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        self.threads = Some(ReadSlot::reading(ticket));
        ticket
    }

    /// Show `tab`. `Some` with the threads' read when their tab is shown
    /// for the first time, or again after a failed read.
    fn select_tab(&mut self, tab: PrTab) -> Option<ReadTicket> {
        self.tab = tab;
        let unread = matches!(self.threads(), None | Some(Load::Failed(_)));
        (tab == PrTab::Threads && unread).then(|| self.begin_threads_read())
    }

    /// Close `job`'s log if open (`None`), else open it with a new read.
    fn toggle_log(&mut self, job: ActionsJob) -> Option<ReadTicket> {
        if self.logs.remove(&job_key(job)).is_some() {
            return None;
        }
        let ticket = self.tickets.issue();
        self.logs.insert(job_key(job), ReadSlot::reading(ticket));
        Some(ticket)
    }

    /// Show `detail` as read, for a capture that needs it in hand.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn show_read(&mut self, detail: PrDetail) {
        self.pr = ReadSlot::settled(Load::Ready(detail));
    }

    /// The read the threads' slot waits on, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn threads_ticket(&self) -> Option<ReadTicket> {
        self.threads.as_ref()?.waiting()
    }

    pub(in crate::workspace) fn threads(&self) -> Option<&Load<ReviewThreads>> {
        self.threads.as_ref().map(ReadSlot::load)
    }

    pub(in crate::workspace) fn log(&self, job: ActionsJob) -> Option<&Load<String>> {
        self.logs.get(&job_key(job)).map(ReadSlot::load)
    }

    /// The read `job`'s open log waits on, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn log_ticket(&self, job: ActionsJob) -> Option<ReadTicket> {
        self.logs.get(&job_key(job))?.waiting()
    }
}

/// A read's answer as its slot keeps it, and the error to report.
fn split<T>(result: Result<T, GhError>) -> (Result<T, String>, Option<GhError>) {
    match result {
        Ok(value) => (Ok(value), None),
        Err(e) => (Err(e.to_string()), Some(e)),
    }
}

impl Workspace {
    pub(in crate::workspace) fn pr_detail_id(&self) -> Option<PrDetailId> {
        self.pages.pull_requests.detail.as_ref().map(|d| d.id)
    }

    pub(super) fn pr_detail_mut(&mut self, id: PrDetailId) -> Option<&mut PrDetailState> {
        self.pages
            .pull_requests
            .detail
            .as_mut()
            .filter(|detail| detail.id == id)
    }

    /// Show `item` over the list. The same pull request already open is
    /// shown as it is; another replaces the detail. Nothing opens while
    /// `[github] enabled` is off, as the list shows no rows then.
    pub(in crate::workspace) fn open_pr_detail(
        &mut self,
        item: PrListItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !crate::github::enabled(cx) {
            return;
        }
        if !self.github_repos().contains(&item.repo) {
            let project = self.lane_scoped.iter().find_map(|(lane, state)| {
                state
                    .github_push()
                    .filter(|(repo, _)| *repo == item.repo)
                    .map(|_| lane.project)
            });
            if let Some(project) = project {
                self.reveal_page_project(project, cx);
            } else {
                self.set_page_scope(crate::workspace::pages::scope::ProjectScope::All, cx);
            }
        }
        let open = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .is_some_and(|d| d.item.repo == item.repo && d.item.pr.number == item.pr.number);
        if open {
            self.show_page(Page::PullRequests, cx);
            return;
        }
        self.leave_page_detail_then(Page::PullRequests, window, cx, move |ws, _, cx| {
            ws.install_pr_detail(item, cx);
        });
    }

    fn install_pr_detail(&mut self, item: PrListItem, cx: &mut Context<Self>) {
        let id = PrDetailId(self.alloc_id());
        self.pages.pull_requests.detail = Some(PrDetailState::new(id, item));
        self.read_pr_detail(id, cx);
        cx.notify();
    }

    /// `[github] enabled` was switched off: the detail closes with the list,
    /// since nothing it shows is kept current and its reads and actions
    /// would still run `gh`.
    pub(crate) fn close_pr_detail_while_off(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.pr_detail_id() {
            self.drop_gone_detail(PageDetailId::PullRequest(id), window, cx);
        }
    }

    /// Back to the list.
    #[cfg(test)]
    pub(in crate::workspace) fn back_to_pr_list(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_page_detail_then(Page::PullRequests, window, cx, |_, _, _| {});
    }

    pub(in crate::workspace) fn refresh_shown_pr_detail(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.pr_detail_id() {
            self.refresh_pr_detail(id, cx);
        }
    }

    /// Read the detail and the list again: the detail's refresh button.
    pub(in crate::workspace) fn refresh_pr_detail(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (read, threads) = detail.reset_for_refresh();
        self.send_pr_detail_read(id, read, cx);
        if let Some(ticket) = threads {
            self.send_pr_threads_read(id, ticket, cx);
        }
        self.refresh_pr_list(cx);
        cx.notify();
    }

    fn read_pr_detail(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        if let Some(ticket) = self.pr_detail_mut(id).map(PrDetailState::begin_read) {
            self.send_pr_detail_read(id, ticket, cx);
        }
    }

    fn send_pr_detail_read(&mut self, id: PrDetailId, ticket: ReadTicket, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_detail(&repo, number),
            move |ws, result, cx| ws.land_pr_detail(id, ticket, result, cx),
        )
        .detach();
    }

    /// A refresh that fails keeps what an earlier read showed.
    pub(in crate::workspace) fn land_pr_detail(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        result: Result<PrDetail, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (answer, error) = split(result);
        if !detail.pr.land_keeping_ready(ticket, answer) {
            return;
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_detail.read", cx);
        }
        cx.notify();
    }

    /// Show `tab`; the threads are read the first time theirs is shown, and
    /// again after a failed read.
    pub(in crate::workspace) fn select_pr_detail_tab(
        &mut self,
        id: PrDetailId,
        tab: PrTab,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if let Some(ticket) = detail.select_tab(tab) {
            self.send_pr_threads_read(id, ticket, cx);
        }
        cx.notify();
    }

    fn send_pr_threads_read(&mut self, id: PrDetailId, ticket: ReadTicket, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.review_threads(&repo, number),
            move |ws, result, cx| ws.land_pr_threads(id, ticket, result, cx),
        )
        .detach();
    }

    /// Only the newest read lands: after a refresh an older answer would
    /// otherwise fill the slot the newer read is waiting in, and a refresh
    /// away from the tab dropped the slot altogether.
    pub(in crate::workspace) fn land_pr_threads(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        result: Result<ReviewThreads, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.pr_detail_mut(id).and_then(|d| d.threads.as_mut()) else {
            return;
        };
        let (answer, error) = split(result);
        if !slot.land(ticket, answer) {
            return;
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_threads.read", cx);
        }
        cx.notify();
    }

    /// Open a failed Actions job's log under its check, reading it the
    /// first time; close it if open.
    pub(in crate::workspace) fn toggle_pr_check_log(
        &mut self,
        id: PrDetailId,
        job: ActionsJob,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(ticket) = detail.toggle_log(job) else {
            cx.notify();
            return;
        };
        let repo = detail.item.repo.clone();
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || {
                gh.failed_log(&repo, job)
                    .map(|raw| daruda_github::prompt::clean_log(&raw))
            },
            move |ws, result, cx| ws.land_pr_check_log(id, job, ticket, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Only the read the open log waits for lands; an earlier read of the
    /// same job, from before a close or a refresh, is dropped.
    pub(in crate::workspace) fn land_pr_check_log(
        &mut self,
        id: PrDetailId,
        job: ActionsJob,
        ticket: ReadTicket,
        result: Result<String, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.logs.get_mut(&job_key(job)))
        else {
            return;
        };
        let (answer, error) = split(result);
        if !slot.land(ticket, answer) {
            return;
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_log.read", cx);
        }
        cx.notify();
    }

    /// The worktree working on the shown pull request.
    pub(in crate::workspace) fn pr_detail_lane(&self, item: &PrListItem) -> Option<LaneFacts> {
        list::linked_lane(item, &self.pr_lanes()).cloned()
    }

    /// Where the detail's Send to agent goes: the linked worktree's chat,
    /// with the pull request as last read. `None` without either.
    fn pr_detail_send_target(&self, id: PrDetailId) -> Option<PrSendTarget> {
        let detail = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.id == id)?;
        let Load::Ready(loaded) = detail.load() else {
            return None;
        };
        let lane = self.pr_detail_lane(&detail.item)?.lane;
        Some(PrSendTarget {
            lane,
            link: PrLink {
                repo: detail.item.repo.clone(),
                pr: loaded.summary.clone(),
            },
        })
    }

    pub(in crate::workspace) fn send_pr_detail_checks(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.pr_detail_send_target(id) {
            self.send_failed_checks_to(target, window, cx);
        }
    }

    pub(in crate::workspace) fn send_pr_detail_threads(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.pr_detail_send_target(id) {
            self.send_review_comments_to(target, window, cx);
        }
    }

    /// Make a worktree for the shown pull request, as its list row would.
    pub(in crate::workspace) fn create_pr_detail_lane(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.id == id)
            .map(|d| d.item.clone());
        if let Some(item) = item {
            self.create_lane_from_pr(item, None, window, cx);
        }
    }

    /// Go to the worktree working on the shown pull request.
    pub(in crate::workspace) fn open_pr_detail_lane(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lane = self
            .pages
            .pull_requests
            .detail
            .as_ref()
            .filter(|d| d.id == id)
            .and_then(|d| self.pr_detail_lane(&d.item));
        if let Some(lane) = lane {
            self.activate_lane(lane.lane, window, cx);
        }
    }

    fn report_pr_read(&mut self, e: &GhError, dedup: &'static str, cx: &mut Context<Self>) {
        let report = ErrorReport::new(error::github_read_failed())
            .severity(ErrorSeverity::Warning)
            .from_error(e)
            .at(file!(), line!())
            .dedup(dedup)
            .build();
        self.report_error(report, cx);
    }
}
