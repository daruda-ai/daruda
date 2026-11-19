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
pub(in crate::workspace) mod handoff;
mod handoff_render;
pub(in crate::workspace) mod line_comments;
pub(in crate::workspace) mod links;
mod links_render;
pub(in crate::workspace) mod progress;
mod read_slot;
pub(in crate::workspace) mod recover;
pub(in crate::workspace) mod render;
mod review_comments_render;
pub(in crate::workspace) mod review_submit;
mod review_submit_render;
pub(in crate::workspace) mod reviewers;
mod reviewers_render;
pub(in crate::workspace) mod stack;
mod stack_render;
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

/// The tabs read when first shown rather than with the pull request.
const LAZY_TABS: [PrTab; 3] = [PrTab::Threads, PrTab::Files, PrTab::Description];

/// A read a tab starts when it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum TabRead {
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
    /// Its native stack, read with it.
    stack: Option<ReadSlot<daruda_github::StackRead>>,
    /// The description being edited, kept across refreshes.
    body_edit: Option<description::BodyEdit>,
    /// The reviewer box and the last change to who is asked.
    reviewer_requests: reviewers::ReviewerRequests,
    /// A comment being written on lines of the diff.
    composer: Option<line_comments::LineComposer>,
    /// The review being written and submitted.
    review: Option<review_submit::ReviewSubmit>,
    /// A handoff to an agent being prepared.
    handoff: Option<handoff::Handoff>,
}

/// What is left of opening a detail once something shows it.
pub(in crate::workspace) enum PrDetailStart {
    /// It was open already: only the tab asked for is shown.
    Shown { tab: Option<PrTab> },
    /// It is new: its pull request is read, or the read made to open it
    /// is landed, and the tab it opens at reads what it shows.
    New {
        read: Option<Box<PrDetail>>,
        tab_read: Option<TabRead>,
    },
}

/// How a detail opens, beyond which pull request.
#[derive(Debug, Default)]
pub(in crate::workspace) struct PrOpening {
    /// The tab to show; `None` keeps where it was left.
    pub(in crate::workspace) tab: Option<PrTab>,
    /// The pull request read just now to open it, landed as a new detail's
    /// read. Fresh because a pull request detail never holds unsaved edits,
    /// so its switch is never held behind a prompt. Boxed: a detail is large.
    pub(in crate::workspace) read: Option<Box<PrDetail>>,
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
            stack: None,
            body_edit: None,
            reviewer_requests: reviewers::ReviewerRequests::default(),
            composer: None,
            review: None,
            handoff: None,
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

    /// Where a detail opens: on `tab` when one was asked for, else where it
    /// was left. A tab asked for that is the one left keeps its place.
    fn open_at(&mut self, place: Option<&progress::Place>, tab: Option<PrTab>) -> Option<TabRead> {
        match (tab, place) {
            (Some(tab), Some(place)) if place.tab == tab => self.restore(place),
            (Some(tab), _) => self.select_tab(tab),
            (None, Some(place)) => self.restore(place),
            (None, None) => None,
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

    pub(in crate::workspace) fn review(&self) -> Option<&review_submit::ReviewSubmit> {
        self.review.as_ref()
    }

    pub(in crate::workspace) fn composer(&self) -> Option<&line_comments::LineComposer> {
        self.composer.as_ref()
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
        if self.action.running().is_some() {
            return None;
        }
        self.pr.last_known()
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

    /// Say under the header that the last action was taken. Called after
    /// the reload, like [`Self::fail_action`].
    pub(super) fn done_action(&mut self, line: String) {
        self.action = ActionState::Done(line);
    }

    /// Forget what a reload replaces: the open logs, a failure already
    /// said, the threads and the files. `Some` with the shown tab's new read
    /// when it had one, so it is read again at once.
    fn reset_for_refresh(&mut self) -> (ReadTicket, Option<TabRead>) {
        self.logs.clear();
        if matches!(self.action, ActionState::Failed(_) | ActionState::Done(_)) {
            self.action = ActionState::Idle;
        }
        // Shown, a tab is read again in place: what it shows stays up until
        // the new answer. Hidden, it is dropped and read when shown again.
        for tab in LAZY_TABS {
            if tab != self.tab {
                self.drop_tab(tab);
            }
        }
        let read = self.begin_read();
        let tab = self.reread_tab(self.tab);
        (read, tab)
    }

    /// The head just read, told to the files: a listing read at another
    /// commit is read again when shown, and dropped when hidden.
    fn files_at_head(&mut self) -> Option<TabRead> {
        let head = self.pr.confirmed()?.head_sha.clone();
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

    /// Show `tab`. `Some` with its read when a tab read on demand is shown
    /// for the first time, or again after a failed read.
    fn select_tab(&mut self, tab: PrTab) -> Option<TabRead> {
        self.tab = tab;
        if !self.tab_wants_read(tab) {
            return None;
        }
        match tab {
            PrTab::Threads => Some(TabRead::Threads(self.begin_threads_read())),
            PrTab::Files => Some(TabRead::Files(self.begin_files_read())),
            PrTab::Description => Some(TabRead::Conversation(self.begin_conversation_read())),
            PrTab::Checks => None,
        }
    }

    /// Whether `tab` has nothing to show yet, or only a failed read.
    fn tab_wants_read(&self, tab: PrTab) -> bool {
        match tab {
            PrTab::Threads => matches!(self.threads(), None | Some(Load::Failed(_))),
            PrTab::Files => self
                .files
                .as_ref()
                .is_none_or(files::FilesState::wants_read),
            PrTab::Description => self.conversation_wanted(),
            PrTab::Checks => false,
        }
    }

    /// Read `tab` again in place, if it holds anything; what it shows stays
    /// until the answer.
    fn reread_tab(&mut self, tab: PrTab) -> Option<TabRead> {
        match tab {
            PrTab::Threads => self.reread_threads().1,
            PrTab::Files => self
                .files
                .is_some()
                .then(|| TabRead::Files(self.reread_files())),
            PrTab::Description => self
                .conversation
                .is_some()
                .then(|| TabRead::Conversation(self.reread_conversation())),
            PrTab::Checks => None,
        }
    }

    /// Let go of what `tab` read; shown again, it is read again.
    fn drop_tab(&mut self, tab: PrTab) {
        match tab {
            PrTab::Threads => self.threads = None,
            PrTab::Files => self.files = None,
            PrTab::Description => self.conversation = None,
            PrTab::Checks => {}
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
        self.pages.pull_requests.detail().map(|d| d.id)
    }

    /// The open detail `id`, wherever it is shown: on the page or in a
    /// worktree's pane. Anything given a detail's id finds it here, never
    /// through the page's shown one.
    pub(in crate::workspace) fn pr_detail(&self, id: PrDetailId) -> Option<&PrDetailState> {
        self.pages.pull_requests.details.get(id)
    }

    pub(super) fn pr_detail_mut(&mut self, id: PrDetailId) -> Option<&mut PrDetailState> {
        self.pages.pull_requests.details.get_mut(id)
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
        self.open_pr_detail_as(item, PrOpening::default(), window, cx);
    }

    /// Open `item`'s detail as `opening` asks, applied as the detail is put up.
    pub(in crate::workspace) fn open_pr_detail_as(
        &mut self,
        item: PrListItem,
        opening: PrOpening,
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
            .detail()
            .filter(|d| d.item.repo == item.repo && d.item.pr.number == item.pr.number)
            .map(|d| d.id);
        if let Some(id) = open {
            self.show_page(Page::PullRequests, cx);
            // The open detail keeps its own reads: `opening.read` started
            // before them could be older than one already landed.
            if let Some(tab) = opening.tab {
                self.select_pr_detail_tab(id, tab, cx);
            }
            return;
        }
        self.leave_page_detail_then(Page::PullRequests, window, cx, move |ws, window, cx| {
            ws.install_pr_detail(item, opening, cx);
            // The list that had the keyboard is no longer drawn.
            if ws.active_page() == Some(Page::PullRequests) {
                ws.focus_shown_page(window, cx);
            }
        });
    }

    /// Put `item`'s detail on the page: the one already open for that
    /// pull request (a worktree's pane holds it), else a new one.
    fn install_pr_detail(&mut self, item: PrListItem, opening: PrOpening, cx: &mut Context<Self>) {
        let (id, reads) = self.pr_detail_for(item, opening);
        self.pages.pull_requests.show_detail(Some(id));
        self.start_pr_detail(id, reads, cx);
        cx.notify();
    }

    /// The open detail of `item`'s pull request, or a new one, with what is
    /// left to do once something shows it. Nothing is read here: a detail
    /// must be on the page or in a pane before its reads start, since one
    /// nothing shows does not stay open.
    pub(in crate::workspace) fn pr_detail_for(
        &mut self,
        item: PrListItem,
        opening: PrOpening,
    ) -> (PrDetailId, PrDetailStart) {
        let page = &mut self.pages.pull_requests;
        if let Some(id) = page.details.of(&item.repo, item.pr.number) {
            // The open detail keeps its own reads: `opening.read` started
            // before them could be older than one already landed.
            return (id, PrDetailStart::Shown { tab: opening.tab });
        }
        let id = PrDetailId(self.alloc_id());
        let mut detail = PrDetailState::new(id, item);
        let page = &mut self.pages.pull_requests;
        let place = page.progress.place(&detail.key()).cloned();
        let tab_read = detail.open_at(place.as_ref(), opening.tab);
        page.details.open(detail);
        (
            id,
            PrDetailStart::New {
                read: opening.read,
                tab_read,
            },
        )
    }

    /// Finish opening detail `id`, now that something shows it.
    pub(in crate::workspace) fn start_pr_detail(
        &mut self,
        id: PrDetailId,
        start: PrDetailStart,
        cx: &mut Context<Self>,
    ) {
        match start {
            PrDetailStart::Shown { tab } => {
                if let Some(tab) = tab {
                    self.select_pr_detail_tab(id, tab, cx);
                }
            }
            PrDetailStart::New { read, tab_read } => {
                crate::github::DraftStore::flush(cx);
                match read {
                    // Read just now to open it: shown as is, not asked again.
                    Some(read) => self.land_pr_read(id, *read, cx),
                    None => self.read_pr_detail(id, cx),
                }
                if let Some(read) = tab_read {
                    self.send_pr_tab_read(id, read, cx);
                }
            }
        }
    }

    /// `[github] enabled` was switched off: the detail closes with the list,
    /// and so does every worktree's pull request pane, since nothing they
    /// show is kept current and their reads and actions would still run `gh`.
    pub(crate) fn close_pr_detail_while_off(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self.pr_detail_id() {
            self.drop_gone_detail(PageDetailId::PullRequest(id), window, cx);
        }
        self.close_pr_panes(window, cx);
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
        self.reprepare_pr_handoff(id, cx);
        self.refresh_pr_list(cx);
        cx.notify();
    }

    /// Show `read`, made elsewhere just now, as the detail's latest read;
    /// its stack is read beside it as with any read.
    fn land_pr_read(&mut self, id: PrDetailId, read: PrDetail, cx: &mut Context<Self>) {
        if let Some(ticket) = self.pr_detail_mut(id).map(PrDetailState::begin_read) {
            self.land_pr_detail(id, ticket, Ok(read), cx);
        }
        self.read_pr_stack(id, cx);
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
        // Its stack with it: apart, so a server without stacks still reads
        // the detail.
        if self
            .pr_detail_mut(id)
            .is_some_and(|detail| detail.stack_worth_reading())
        {
            self.read_pr_stack(id, cx);
        }
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
        detail.drop_handoff_to_fix();
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
        // A failed refresh leaves the threads shown up; a reply waiting on it
        // stays unconfirmed, not checked against the threads read before.
        if !slot.land_keeping_ready(ticket, answer) {
            return;
        }
        // Threads resolved since are no longer expanded, and work on them
        // settles against what was read.
        let read = match slot.load() {
            Load::Ready(threads) if error.is_none() => Some(threads.clone()),
            Load::Ready(_) | Load::Loading | Load::Failed(_) => None,
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
        let settled = detail.thread_work.threads_read(ticket, read.as_ref());
        for (thread, how) in settled {
            match how {
                thread_actions::Settled::Posted => self.set_pr_reply_draft(id, &thread, None, cx),
                thread_actions::Settled::NotPosted => self.let_go_pr_reply_sent(id, &thread, cx),
            }
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
    pub(super) fn pr_detail_send_target(&self, id: PrDetailId) -> Option<PrSendTarget> {
        let detail = self.pr_detail(id)?;
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

    /// Make a worktree for the shown pull request, as its list row would.
    pub(in crate::workspace) fn create_pr_detail_lane(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item = self.pr_detail(id).map(|d| d.item.clone());
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
            .pr_detail(id)
            .and_then(|d| self.pr_detail_lane(&d.item));
        if let Some(lane) = lane {
            self.activate_lane(lane.lane, window, cx);
        }
    }

    #[track_caller]
    pub(super) fn report_pr_read(
        &mut self,
        e: &GhError,
        dedup: &'static str,
        cx: &mut Context<Self>,
    ) {
        let at = std::panic::Location::caller();
        let report = ErrorReport::new(error::github_read_failed())
            .severity(ErrorSeverity::Warning)
            .from_error(e)
            .at(at.file(), at.line())
            .dedup(dedup)
            .build();
        self.report_error(report, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::progress::{Place, PlaceFile};
    use super::{PrDetailId, PrDetailState, PrTab};

    fn place(tab: PrTab) -> Place {
        Place {
            tab,
            file: Some(PlaceFile {
                path: "src/a.rs".into(),
                blob: None,
            }),
            offset: (0.0, -40.0),
        }
    }

    /// A tab asked for wins over where the detail was left; asked for the
    /// tab it was left on, it comes back to the file there too.
    #[test]
    fn a_tab_asked_for_opens_and_keeps_the_place_only_on_its_own_tab() {
        let item = crate::workspace::tests::pull_request_detail::item(7, "feat");
        let cases = [
            (
                "no tab: where it was left",
                None,
                PrTab::Files,
                PrTab::Files,
                true,
            ),
            (
                "another tab: that tab",
                Some(PrTab::Checks),
                PrTab::Files,
                PrTab::Checks,
                false,
            ),
            (
                "the same tab: its place too",
                Some(PrTab::Files),
                PrTab::Files,
                PrTab::Files,
                true,
            ),
        ];
        for (label, tab, left_on, shown, restored) in cases {
            let mut state = PrDetailState::new(PrDetailId(1), item.clone());
            state.open_at(Some(&place(left_on)), tab);
            assert_eq!(state.tab, shown, "{label}");
            let chosen = state
                .files
                .as_ref()
                .is_some_and(|files| files.restoring_to("src/a.rs"));
            assert_eq!(chosen, restored, "{label}");
        }
    }
}
