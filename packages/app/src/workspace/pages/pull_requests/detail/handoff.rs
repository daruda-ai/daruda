//! Handing part of a pull request to an agent from the detail: what is
//! sent (failing checks, open threads, selected lines), where (a writable agent chat
//! in the linked worktree, chosen among them), and the prompt itself,
//! prepared and shown before anything reaches a chat. Inserted into the
//! chat's input unsent; the chat is checked again then.

mod delivery;

use daruda_github::GhError;
use daruda_store::project::LaneRef;
use gpui::{Context, Window};

use super::diff_layout::{LineRange, Side};
use super::read_slot::ReadTicket;
use super::{Load, PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::github::PrLink;
use crate::workspace::Workspace;
use crate::workspace::github_ops::review_send::{checks_prompt, threads_prompt};
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// What is handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum Payload {
    /// The failing checks, with their logs.
    FailedChecks,
    FailedJob(daruda_github::ActionsJob),
    /// The unresolved review threads.
    OpenThreads,
    /// These lines selected in the Files tab, quoted as they were then.
    SelectedCode(LineRange),
}

impl Payload {
    /// What is read through `gh` to prepare it; `None` when it is quoted
    /// from the page as it is.
    fn read(&self) -> Option<Read> {
        match self {
            Self::FailedChecks => Some(Read::FailedChecks),
            Self::FailedJob(job) => Some(Read::FailedJob(*job)),
            Self::OpenThreads => Some(Read::OpenThreads),
            Self::SelectedCode(_) => None,
        }
    }
}

/// What is read through `gh` to prepare a handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Read {
    FailedChecks,
    FailedJob(daruda_github::ActionsJob),
    OpenThreads,
}

impl From<Read> for Payload {
    fn from(read: Read) -> Self {
        match read {
            Read::FailedChecks => Self::FailedChecks,
            Read::FailedJob(job) => Self::FailedJob(job),
            Read::OpenThreads => Self::OpenThreads,
        }
    }
}

/// Where preparing the prompt stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum Prepared {
    Reading(ReadTicket),
    Ready(String),
    /// Nothing to hand over: no failing check, no open thread.
    Nothing,
    Failed(String),
}

/// The worktree a handoff was shown with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum HandoffLane {
    /// None yet: one can be made for it.
    Unlinked,
    /// The worktree the pull request was linked to.
    Linked(LaneRef),
    /// Made by this detail's checkout for the pull request. Its link is read
    /// from the new worktree's branch later, so until then it is taken as is.
    Made(LaneRef),
}

/// Where a handoff goes, against the worktree the pull request links now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Destination {
    /// No worktree: one is made first.
    Checkout,
    To(LaneRef),
    /// The pull request now links another worktree than the one shown.
    Moved,
}

impl Destination {
    pub(in crate::workspace) fn lane(self) -> Option<LaneRef> {
        match self {
            Self::To(lane) => Some(lane),
            Self::Checkout | Self::Moved => None,
        }
    }
}

/// A handoff being prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Handoff {
    /// A new preview can quote exactly the same text as one just cancelled.
    id: ReadTicket,
    pub payload: Payload,
    /// The worktree whose chats can take it, and its name then.
    lane: HandoffLane,
    pub lane_name: String,
    /// The chat chosen; `None` while the worktree has none.
    pub chat: Option<PaneId>,
    pub prompt: Prepared,
    pub editor: Option<gpui::Entity<crate::ui::TextareaState>>,
}

impl Handoff {
    /// Where it goes now that the pull request links `linked`. A worktree
    /// linked since it was shown without one is taken; one linked other than
    /// the worktree it was shown with is a move.
    pub(in crate::workspace) fn destination(&self, linked: Option<LaneRef>) -> Destination {
        match (self.lane, linked) {
            (HandoffLane::Unlinked, None) => Destination::Checkout,
            (HandoffLane::Unlinked, Some(now)) => Destination::To(now),
            (HandoffLane::Linked(lane) | HandoffLane::Made(lane), Some(now)) if now == lane => {
                Destination::To(lane)
            }
            (HandoffLane::Made(lane), None) => Destination::To(lane),
            (HandoffLane::Linked(_) | HandoffLane::Made(_), _) => Destination::Moved,
        }
    }

    /// The worktree it was shown with or made for it, if any.
    #[cfg(test)]
    pub(in crate::workspace) fn lane(&self) -> Option<LaneRef> {
        match self.lane {
            HandoffLane::Unlinked => None,
            HandoffLane::Linked(lane) | HandoffLane::Made(lane) => Some(lane),
        }
    }

    fn draft(&self, cx: &gpui::App) -> Option<String> {
        let Prepared::Ready(prepared) = &self.prompt else {
            return None;
        };
        let text = self.editor.as_ref().map_or_else(
            || prepared.clone(),
            |edit| edit.read(cx).value().to_string(),
        );
        (!text.trim().is_empty()).then_some(text)
    }
}

impl PrDetailState {
    pub(super) fn wait_handoff_for_detail(&mut self, ticket: ReadTicket) {
        if let Some(held) = self.handoff.as_mut()
            && held.editor.is_none()
            && held.payload.read().is_some()
        {
            held.prompt = Prepared::Reading(ticket);
        }
    }

    pub(in crate::workspace) fn handoff(&self) -> Option<&Handoff> {
        self.handoff.as_ref()
    }

    /// Read closed or merged: a handoff of its failing checks or open
    /// threads is closed, as nobody is left to fix them. Read with the
    /// checks it quotes passing, it is closed too, edited or not: a prompt
    /// about a failure no longer there must not reach a chat. The logs of
    /// jobs no longer failing go with it.
    pub(super) fn drop_handoff_to_fix(&mut self) {
        let Some(pr) = self.pr.confirmed() else {
            return;
        };
        // A truncated list may leave a failing job off its first page.
        let failing = |job: daruda_github::ActionsJob| {
            pr.checks_truncated
                || pr.summary.failed.iter().any(|check| check.job == Some(job))
                || pr.checks.iter().any(|entry| {
                    entry.job == Some(job)
                        && entry.run.status == daruda_github::CheckStatus::Failure
                })
        };
        self.logs
            .retain(|&(run, job), _| failing(daruda_github::ActionsJob { run, job }));
        let stale = match self.handoff.as_ref().map(|handoff| &handoff.payload) {
            Some(Payload::FailedJob(job)) => !failing(*job),
            Some(Payload::FailedChecks) => {
                !pr.checks_truncated
                    && pr.summary.failed.is_empty()
                    && !pr
                        .checks
                        .iter()
                        .any(|entry| entry.run.status == daruda_github::CheckStatus::Failure)
            }
            Some(Payload::OpenThreads | Payload::SelectedCode(_)) | None => false,
        };
        let closed = !pr.summary.state.is_active()
            && self
                .handoff
                .as_ref()
                .is_some_and(|handoff| handoff.payload.read().is_some());
        if stale || closed {
            self.handoff = None;
        }
    }
}

impl Workspace {
    /// Prepare what `read` names, read through `gh`, for an agent in the
    /// shown pull request's worktree.
    pub(in crate::workspace) fn open_pr_handoff(
        &mut self,
        id: PrDetailId,
        read: Read,
        cx: &mut Context<Self>,
    ) {
        let payload = Payload::from(read);
        if self
            .pr_detail(id)
            .and_then(|d| d.handoff())
            .is_some_and(|held| {
                held.payload == payload
                    && matches!(held.prompt, Prepared::Ready(_) | Prepared::Reading(_))
            })
        {
            return;
        }
        if let Some(ticket) = self.pr_detail(id).and_then(|detail| detail.pr.waiting()) {
            self.begin_pr_handoff(id, payload, Prepared::Reading(ticket), cx);
            return;
        }
        // The open log is already the user's chosen source. Quote those bytes;
        // a refresh can fetch a fresh copy through the normal preparation path.
        if let Read::FailedJob(job) = read
            && let Some(detail) = self.pr_detail(id)
            && let Some(Load::Ready(log)) = detail.log(job)
            && let Some(pr) = detail.pr.last_known()
            && let Some(check) = pr
                .summary
                .failed
                .iter()
                .find(|check| check.job == Some(job))
        {
            let log = daruda_github::prompt::CheckLog {
                check: check.clone(),
                log: Ok(log.clone()),
            };
            let prompt = daruda_github::prompt::failed_checks(&pr.summary, &[log]);
            self.begin_pr_handoff(id, payload, Prepared::Ready(prompt), cx);
            return;
        }
        let Some(ticket) = self.pr_detail_mut(id).map(|detail| detail.tickets.issue()) else {
            return;
        };
        let Some(target) = self.begin_pr_handoff(id, payload, Prepared::Reading(ticket), cx) else {
            return;
        };
        self.send_pr_handoff_read(id, read, ticket, target, cx);
    }

    /// The pull request was read again: a handoff of what is read through
    /// `gh` is prepared again with it, keeping the chat chosen, so what goes
    /// in is never older than the page. Quoted lines are checked as they go.
    pub(super) fn reprepare_pr_handoff(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        error: Option<&GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(read) = self
            .pr_detail(id)
            .and_then(|detail| detail.handoff.as_ref())
            .filter(|held| held.prompt == Prepared::Reading(ticket))
            .and_then(|held| held.payload.read())
        else {
            return;
        };
        // A failed refresh keeps the pull request shown, so the prompt is
        // prepared from that; only with nothing ever read does it fail.
        let Some(link) = self.pr_handoff_link(id) else {
            if let (Some(error), Some(held)) = (
                error,
                self.pr_detail_mut(id).and_then(|d| d.handoff.as_mut()),
            ) {
                held.prompt = Prepared::Failed(error.to_string());
                cx.notify();
            }
            return;
        };
        self.send_pr_handoff_read(id, read, ticket, link, cx);
    }

    fn send_pr_handoff_read(
        &mut self,
        id: PrDetailId,
        read: Read,
        ticket: ReadTicket,
        link: PrLink,
        cx: &mut Context<Self>,
    ) {
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || build(read, gh.as_ref(), &link),
            move |ws, built, cx| ws.land_pr_handoff(id, ticket, built, cx),
        )
        .detach();
        cx.notify();
    }

    /// Prepare the lines selected in the Files tab for an agent in the
    /// shown pull request's worktree; nothing to read, so it is ready at once.
    pub(in crate::workspace) fn open_pr_code_handoff(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some((range, code, complete)) = detail
            .files
            .as_ref()
            .and_then(|files| files.selection_code())
        else {
            return;
        };
        // The same lines already shown keep the chat chosen for them.
        if detail
            .handoff
            .as_ref()
            .is_some_and(|held| held.payload == Payload::SelectedCode(range.clone()))
        {
            return;
        }
        let Some(pr) = detail.pr.last_known().map(|pr| pr.summary.clone()) else {
            return;
        };
        let lines = daruda_github::prompt::CodeLines {
            reference: &range.reference(),
            code: &code,
            complete,
            removed: range.side == Side::Old,
        };
        let prompt = daruda_github::prompt::selected_code(&pr, &lines);
        self.begin_pr_handoff(
            id,
            Payload::SelectedCode(range),
            Prepared::Ready(prompt),
            cx,
        );
    }

    /// Show a handoff of `payload` to the linked worktree's chats, with
    /// `prompt` as it stands; `None` without a linked worktree.
    fn begin_pr_handoff(
        &mut self,
        id: PrDetailId,
        payload: Payload,
        prompt: Prepared,
        cx: &mut Context<Self>,
    ) -> Option<PrLink> {
        let link = self.pr_handoff_link(id)?;
        let item = self.pr_detail(id)?.item();
        let (lane, lane_name) = match self.pr_detail_lane(item) {
            Some(lane) => (HandoffLane::Linked(lane.lane), lane.name),
            None => {
                let (_, push) = self.pr_lane_home(&item.repo, None)?;
                (
                    HandoffLane::Unlinked,
                    daruda_github::PrCheckout::of(item, &push.remote).branch,
                )
            }
        };
        let chat = match lane {
            HandoffLane::Linked(lane) => {
                self.writable_chats(lane, cx).first().map(|chat| chat.pane)
            }
            HandoffLane::Unlinked | HandoffLane::Made(_) => None,
        };
        let detail = self.pr_detail_mut(id)?;
        detail.handoff = Some(Handoff {
            id: detail.tickets.issue(),
            payload,
            lane,
            lane_name,
            chat,
            prompt,
            editor: None,
        });
        cx.notify();
        Some(link)
    }

    /// Where detail `id`'s handoff goes, against the worktree its pull
    /// request links now: the one test the view and every insert share.
    fn pr_handoff_destination(&self, id: PrDetailId) -> Option<Destination> {
        let detail = self.pr_detail(id)?;
        let linked = self.pr_detail_lane(&detail.item).map(|lane| lane.lane);
        Some(detail.handoff.as_ref()?.destination(linked))
    }

    fn pr_handoff_link(&self, id: PrDetailId) -> Option<PrLink> {
        let detail = self.pr_detail(id)?;
        Some(PrLink {
            repo: detail.item.repo.clone(),
            pr: detail.pr.last_known()?.summary.clone(),
        })
    }

    fn land_pr_handoff(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        built: Result<Option<String>, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(handoff) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.handoff.as_mut())
            .filter(|handoff| handoff.prompt == Prepared::Reading(ticket))
        else {
            return;
        };
        handoff.prompt = match built {
            Ok(Some(prompt)) => Prepared::Ready(prompt),
            Ok(None) => Prepared::Nothing,
            Err(e) => Prepared::Failed(e.to_string()),
        };
        cx.notify();
    }

    pub(in crate::workspace) fn choose_pr_handoff_chat(
        &mut self,
        id: PrDetailId,
        chat: PaneId,
        cx: &mut Context<Self>,
    ) {
        if let Some(handoff) = self.pr_detail_mut(id).and_then(|d| d.handoff.as_mut()) {
            handoff.chat = Some(chat);
            cx.notify();
        }
    }

    pub(in crate::workspace) fn close_pr_handoff(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.handoff = None;
            cx.notify();
        }
    }

    pub(in crate::workspace) fn edit_pr_handoff(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(held) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.handoff.as_mut())
        else {
            return;
        };
        let Prepared::Ready(prompt) = &held.prompt else {
            return;
        };
        let input = crate::ui::make_markdown_prose_state(
            prompt,
            "",
            crate::ui::theme::PR_BODY_EDIT_ROWS,
            window,
            cx,
        );
        cx.subscribe(&input, |_, _, event: &crate::ui::InputEvent, cx| {
            if matches!(event, crate::ui::InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        input.update(cx, |input, cx| input.focus(window, cx));
        held.editor = Some(input);
        cx.notify();
    }
}

fn build(read: Read, gh: &dyn daruda_github::Gh, link: &PrLink) -> Result<Option<String>, GhError> {
    match read {
        Read::FailedChecks => checks_prompt(gh, link),
        Read::FailedJob(job) => {
            let mut single = link.clone();
            single.pr.failed.retain(|check| check.job == Some(job));
            checks_prompt(gh, &single)
        }
        Read::OpenThreads => threads_prompt(gh, link),
    }
}
