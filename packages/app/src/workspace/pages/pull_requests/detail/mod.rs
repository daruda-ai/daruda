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

pub(in crate::workspace) mod description;
mod description_render;
pub(in crate::workspace) mod diff_layout;
mod diff_render;
pub(in crate::workspace) mod files;
pub(in crate::workspace) mod files_render;
pub(in crate::workspace) mod progress;
mod read_slot;
pub(in crate::workspace) mod recover;
pub(in crate::workspace) mod render;
pub(in crate::workspace) mod reviewers;
mod reviewers_render;
pub(in crate::workspace) mod thread_actions;
mod threads_render;
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
    Files,
    Threads,
    Description,
}

impl PrTab {
    pub(in crate::workspace) const ALL: [Self; 4] =
        [Self::Checks, Self::Files, Self::Threads, Self::Description];
}

/// A read a tab starts when it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabRead {
    Threads(ReadTicket),
    Files(ReadTicket),
    Conversation(ReadTicket),
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
    /// The Files tab's listing, kept with the detail so stepping through
    /// files can bring the shown one into view.
    files_scroll: gpui::UniformListScrollHandle,
    /// The unresolved threads, read when their tab is first shown.
    threads: Option<ReadSlot<ReviewThreads>>,
    /// The changed files, read when their tab is first shown.
    files: Option<files::FilesState>,
    /// The failed Actions jobs whose log is open.
    logs: BTreeMap<JobKey, ReadSlot<String>>,
    /// What it is doing to the pull request on GitHub, or what failed.
    action: ActionState,
    /// Every read this detail sends, whichever slot it fills.
    tickets: Tickets,
    /// Review threads shown in full, by GitHub id.
    expanded_threads: std::collections::HashSet<String>,
    /// Replies being written and actions on threads.
    thread_work: thread_actions::ThreadsWork,
    /// The comments on the pull request itself, read when the Description
    /// tab is first shown.
    conversation: Option<ReadSlot<daruda_github::Conversation>>,
    /// The description being edited, kept across refreshes.
    body_edit: Option<description::BodyEdit>,
    /// The reviewer box and the last change to who is asked.
    reviewer_requests: reviewers::ReviewerRequests,
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
            files_scroll: gpui::UniformListScrollHandle::new(),
            threads: None,
            files: None,
            logs: BTreeMap::new(),
            action: ActionState::Idle,
            tickets: Tickets::default(),
            expanded_threads: std::collections::HashSet::new(),
            thread_work: thread_actions::ThreadsWork::default(),
            conversation: None,
            body_edit: None,
            reviewer_requests: reviewers::ReviewerRequests::default(),
        }
    }

    pub(in crate::workspace) fn id(&self) -> PrDetailId {
        self.id
    }

    pub(in crate::workspace) fn item(&self) -> &PrListItem {
        &self.item
    }

    pub(in crate::workspace) fn key(&self) -> super::list::PrKey {
        (self.item.repo.clone(), self.item.pr.number)
    }

    /// Where it stands, to come back to when the same pull request is
    /// opened again.
    pub(super) fn place(&self) -> progress::Place {
        // Left before coming back: the place come back to still stands.
        if let Some((file, offset)) = self
            .files
            .as_ref()
            .and_then(files::FilesState::pending_place)
        {
            return progress::Place {
                tab: PrTab::Files,
                file: Some(file),
                offset,
            };
        }
        let offset = self.scroll.offset();
        progress::Place {
            tab: self.tab,
            file: self.files.as_ref().and_then(files::FilesState::place_file),
            offset: (offset.x.into(), offset.y.into()),
        }
    }

    /// Come back to `place`: its tab, and on the Files tab the file shown,
    /// with where the detail was scrolled if that file did not change.
    /// `Some` with the read the tab needs.
    fn restore(&mut self, place: &progress::Place) -> Option<TabRead> {
        let read = self.select_tab(place.tab);
        if let (Some(files), Some(file)) = (self.files.as_mut(), place.file.as_ref()) {
            files.restore_to(file, place.offset);
        }
        read
    }

    pub(in crate::workspace) fn scroll(&self) -> &gpui::ScrollHandle {
        &self.scroll
    }

    pub(in crate::workspace) fn thread_work(&self) -> &thread_actions::ThreadsWork {
        &self.thread_work
    }

    pub(in crate::workspace) fn thread_expanded(&self, thread: &str) -> bool {
        self.expanded_threads.contains(thread)
    }

    pub(in crate::workspace) fn files_scroll(&self) -> &gpui::UniformListScrollHandle {
        &self.files_scroll
    }

    /// The pull request's read out, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn pr_waiting(&self) -> Option<ReadTicket> {
        self.pr.waiting()
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
    /// said, the threads and the files. `Some` with the shown tab's new read
    /// when it had one, so it is read again at once.
    fn reset_for_refresh(&mut self) -> (ReadTicket, Option<TabRead>) {
        self.logs.clear();
        if matches!(self.action, ActionState::Failed(_)) {
            self.action = ActionState::Idle;
        }
        let threads = self.threads.take().is_some() && self.tab == PrTab::Threads;
        // Shown, the conversation is read again in place; hidden, dropped
        // and read when shown again.
        let conversation = if self.tab == PrTab::Description {
            self.conversation.is_some()
        } else {
            self.conversation = None;
            false
        };
        // Shown, the files are read again in place: the listing and the
        // chosen file stay up until the new answer. Hidden, they are dropped
        // and read when shown again.
        let files = if self.tab == PrTab::Files {
            self.files.is_some()
        } else {
            self.files = None;
            false
        };
        let read = self.begin_read();
        let tab = if threads {
            Some(TabRead::Threads(self.begin_threads_read()))
        } else if conversation {
            Some(TabRead::Conversation(self.reread_conversation()))
        } else if files {
            Some(TabRead::Files(self.reread_files()))
        } else {
            None
        };
        (read, tab)
    }

    /// The head just read, told to the files: a listing read at another
    /// commit is read again when shown, and dropped when hidden.
    fn files_at_head(&mut self) -> Option<TabRead> {
        let head = self.pr.current()?.head_sha.clone();
        if !self.files.as_mut()?.head_read(&head) {
            return None;
        }
        if self.tab == PrTab::Files {
            Some(TabRead::Files(self.reread_files()))
        } else {
            self.files = None;
            None
        }
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

    /// Show `tab`. `Some` with its read when the threads or the files are
    /// shown for the first time, or again after a failed read.
    fn select_tab(&mut self, tab: PrTab) -> Option<TabRead> {
        self.tab = tab;
        match tab {
            PrTab::Threads if matches!(self.threads(), None | Some(Load::Failed(_))) => {
                Some(TabRead::Threads(self.begin_threads_read()))
            }
            PrTab::Files
                if self
                    .files
                    .as_ref()
                    .is_none_or(files::FilesState::wants_read) =>
            {
                Some(TabRead::Files(self.begin_files_read()))
            }
            PrTab::Description if self.conversation_wanted() => {
                Some(TabRead::Conversation(self.begin_conversation_read()))
            }
            PrTab::Checks | PrTab::Files | PrTab::Threads | PrTab::Description => None,
        }
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
        self.select_pr_row((item.repo.clone(), item.pr.number));
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
        self.leave_page_detail_then(Page::PullRequests, window, cx, move |ws, window, cx| {
            ws.install_pr_detail(item, cx);
            // The list that had the keyboard is no longer drawn.
            if ws.active_page() == Some(Page::PullRequests) {
                ws.focus_shown_page(window, cx);
            }
        });
    }

    fn install_pr_detail(&mut self, item: PrListItem, cx: &mut Context<Self>) {
        let id = PrDetailId(self.alloc_id());
        let mut detail = PrDetailState::new(id, item);
        let place = self
            .pages
            .pull_requests
            .progress
            .place(&detail.key())
            .cloned();
        let tab_read = place.and_then(|place| detail.restore(&place));
        self.pages.pull_requests.replace_detail(Some(detail));
        self.read_pr_detail(id, cx);
        if let Some(read) = tab_read {
            self.send_pr_tab_read(id, read, cx);
        }
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
        let (read, tab) = detail.reset_for_refresh();
        self.send_pr_detail_read(id, read, cx);
        if let Some(tab) = tab {
            self.send_pr_tab_read(id, tab, cx);
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
        detail.come_back();
        if let Some(read) = detail.files_at_head() {
            self.send_pr_tab_read(id, read, cx);
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_detail.read", cx);
        }
        cx.notify();
    }

    /// Show review thread `thread` in full, or fold it back to its excerpt.
    pub(in crate::workspace) fn toggle_pr_thread(
        &mut self,
        id: PrDetailId,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if !detail.expanded_threads.remove(thread) {
            detail.expanded_threads.insert(thread.to_owned());
        }
        cx.notify();
    }

    /// Go to the lines a thread is on: the Files tab, its file, the lines
    /// selected once the file is shown at a known commit.
    pub(in crate::workspace) fn show_pr_thread_in_diff(
        &mut self,
        id: PrDetailId,
        target: view::ThreadTarget,
        cx: &mut Context<Self>,
    ) {
        self.select_pr_detail_tab(id, PrTab::Files, cx);
        let Some(files) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) else {
            return;
        };
        files.go_to_lines(target);
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
        detail.stop_coming_back();
        if let Some(read) = detail.select_tab(tab) {
            self.send_pr_tab_read(id, read, cx);
        }
        cx.notify();
    }

    fn send_pr_tab_read(&mut self, id: PrDetailId, read: TabRead, cx: &mut Context<Self>) {
        match read {
            TabRead::Threads(ticket) => self.send_pr_threads_read(id, ticket, cx),
            TabRead::Files(ticket) => self.send_pr_files_read(id, ticket, 1, cx),
            TabRead::Conversation(ticket) => self.send_pr_conversation_read(id, ticket, cx),
        }
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
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(slot) = detail.threads.as_mut() else {
            return;
        };
        let (answer, error) = split(result);
        if !slot.land(ticket, answer) {
            return;
        }
        // Threads resolved since are no longer expanded, and work on them
        // settles against what was read.
        let read = match slot.load() {
            Load::Ready(threads) => Some(threads.clone()),
            Load::Loading | Load::Failed(_) => None,
        };
        if let Some(threads) = &read {
            let open: std::collections::HashSet<&str> = threads
                .threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect();
            detail
                .expanded_threads
                .retain(|thread| open.contains(thread.as_str()));
        }
        detail
            .thread_work
            .threads_read(ticket, read.as_ref(), |reply| {
                reply.read(cx).value().to_string()
            });
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

    pub(super) fn report_pr_read(
        &mut self,
        e: &GhError,
        dedup: &'static str,
        cx: &mut Context<Self>,
    ) {
        let report = ErrorReport::new(error::github_read_failed())
            .severity(ErrorSeverity::Warning)
            .from_error(e)
            .at(file!(), line!())
            .dedup(dedup)
            .build();
        self.report_error(report, cx);
    }
}
