//! The Pull requests page's detail: one pull request's checks, review
//! threads and description, shown over the list. It holds no edits, so
//! leaving it never asks. Reads run on the background executor and land
//! only while the detail that asked is still the one shown.

use std::collections::BTreeMap;

use daruda_github::{GhError, PrDetail, PrListItem, ReviewThreads};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{App, Context, Window};

use super::actions::{ActionState, PrAction};
use super::list::{self, LaneFacts};

pub(in crate::workspace) mod anchors;
mod checks;
mod checks_render;
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
mod opening;
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
mod thread_reads;
mod threads_render;
pub(in crate::workspace) mod view;

pub(in crate::workspace) use checks::ChecksShown;
use checks::JobKey;
pub(in crate::workspace) use opening::{OpeningTarget, PrOpening};
use read_slot::Tickets;
pub(in crate::workspace) use read_slot::{ReadSlot, ReadTicket};

use crate::gh_host::GhHost;
use crate::surface::strings::error;
use crate::workspace::Workspace;
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
    /// Where a pull request never visited opens, however it is opened.
    #[default]
    Description,
    Checks,
    Files,
    Threads,
}

impl PrTab {
    fn uses(self, source: Self) -> bool {
        self == source || (self == Self::Files && source == Self::Threads)
    }

    pub(in crate::workspace) const ALL: [Self; 4] =
        [Self::Description, Self::Checks, Self::Files, Self::Threads];
}

/// Sources read on demand by the detail tabs.
const LAZY_TABS: [PrTab; 3] = [PrTab::Threads, PrTab::Files, PrTab::Description];

/// A read a tab starts when it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum TabRead {
    Threads(ReadTicket),
    Files(ReadTicket),
    Conversation(ReadTicket),
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
    /// Which checks the Checks tab lists.
    checks_shown: ChecksShown,
    comment_location: Option<anchors::CommentLocation>,
    scroll: gpui::ScrollHandle,
    /// The Files tab's listing, kept with the detail so stepping through
    /// files can bring the shown one into view.
    files_scroll: gpui::UniformListScrollHandle,
    /// The unresolved threads, read when their tab is first shown.
    threads: Option<ReadSlot<ReviewThreads>>,
    /// The detail head paired with the last successful thread read. As with
    /// the files listing, a concurrent detail read supplies it on landing.
    thread_head: Option<String>,
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

impl PrDetailState {
    /// A detail for `item`, its pull request not read yet.
    pub(super) fn new(id: PrDetailId, item: PrListItem) -> Self {
        Self {
            id,
            item,
            pr: ReadSlot::settled(Load::Loading),
            tab: PrTab::default(),
            checks_shown: ChecksShown::default(),
            comment_location: None,
            scroll: gpui::ScrollHandle::new(),
            files_scroll: gpui::UniformListScrollHandle::new(),
            threads: None,
            thread_head: None,
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

    /// Drop hidden sources and reread the visible ones in place.
    fn reset_for_refresh(&mut self) -> (ReadTicket, Vec<TabRead>) {
        self.logs.clear();
        if matches!(self.action, ActionState::Failed(_) | ActionState::Done(_)) {
            self.action = ActionState::Idle;
        }
        // Shown, a tab is read again in place: what it shows stays up until
        // the new answer. Hidden, it is dropped and read when shown again.
        for tab in LAZY_TABS {
            if !self.tab.uses(tab) {
                self.drop_tab(tab);
            }
        }
        let read = self.begin_read();
        let shown = self.tab;
        let tabs = LAZY_TABS
            .into_iter()
            .filter(|source| shown.uses(*source))
            .filter_map(|source| self.reread_tab(source))
            .collect();
        (read, tabs)
    }

    /// The head just read, told to the files: a listing read at another
    /// commit is read again when shown, and dropped when hidden.
    fn files_at_head(&mut self) -> Option<TabRead> {
        let pr = self.pr.confirmed()?;
        let (head, base) = (pr.head_sha.clone(), pr.base_branch.clone());
        let files = self.files.as_mut()?;
        // Both are told, whichever moved.
        if !(files.head_read(&head) | files.base_read(&base)) {
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
        self.wait_handoff_for_detail(ticket);
        ticket
    }

    fn begin_threads_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        self.threads = Some(ReadSlot::reading(ticket));
        ticket
    }

    /// Start each missing read for the chosen tab. Files shares its review
    /// thread read with Threads, including when a detail opens at a saved file.
    fn select_tab(&mut self, tab: PrTab) -> Vec<TabRead> {
        self.tab = tab;
        self.checks_shown = ChecksShown::default();
        let mut reads = Vec::new();
        for source in LAZY_TABS {
            if tab.uses(source) && self.tab_wants_read(source) {
                reads.push(match source {
                    PrTab::Threads => TabRead::Threads(self.begin_threads_read()),
                    PrTab::Files => TabRead::Files(self.begin_files_read()),
                    PrTab::Description => TabRead::Conversation(self.begin_conversation_read()),
                    PrTab::Checks => continue,
                });
            }
        }
        reads
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
            PrTab::Threads => {
                self.threads = None;
                self.thread_head = None;
            }
            PrTab::Files => self.files = None,
            PrTab::Description => self.conversation = None,
            PrTab::Checks => {}
        }
    }

    /// Show `detail` as read, for a capture that needs it in hand.
    #[cfg(feature = "screenshot")]
    pub(in crate::workspace) fn show_read(&mut self, detail: PrDetail) {
        self.pr = ReadSlot::settled(Load::Ready(detail));
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
        let (read, tabs) = detail.reset_for_refresh();
        self.send_pr_detail_read(id, read, cx);
        for tab in tabs {
            self.send_pr_tab_read(id, tab, cx);
        }
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
        if error.is_none()
            && detail.thread_head.is_none()
            && detail
                .threads
                .as_ref()
                .and_then(ReadSlot::confirmed)
                .is_some()
        {
            detail.thread_head = detail.pr.confirmed().map(|pr| pr.head_sha.clone());
        }
        detail.come_back();
        detail.drop_handoff_to_fix();
        if let Some(read) = detail.files_at_head() {
            self.send_pr_tab_read(id, read, cx);
        }
        self.reprepare_pr_handoff(id, ticket, error.as_ref(), cx);
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_detail.read", cx);
        }
        self.follow_pr_comment(id, cx);
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
        detail.comment_location = None;
        detail.stop_coming_back();
        for read in detail.select_tab(tab) {
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

    /// The worktree working on the shown pull request.
    pub(in crate::workspace) fn pr_detail_lane(&self, item: &PrListItem) -> Option<LaneFacts> {
        list::linked_lane(item, &self.pr_lanes()).cloned()
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

    /// Go to the worktree working on the shown pull request, and take the
    /// pull request along: beside the worktree's tabs, where one opened
    /// from its Git panel would go.
    pub(in crate::workspace) fn open_pr_detail_lane(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail(id) else {
            return;
        };
        let item = detail.item().clone();
        let Some(lane) = self.pr_detail_lane(&detail.item) else {
            return;
        };
        self.activate_lane(lane.lane, window, cx);
        let entry = crate::workspace::main_area::tab_ops::PaneEntry::Enter;
        if self.active == lane.lane && self.group_facts().side_only_goes_beside(entry) {
            self.open_pr_pane(lane.lane, item, PrOpening::default(), window, cx);
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
