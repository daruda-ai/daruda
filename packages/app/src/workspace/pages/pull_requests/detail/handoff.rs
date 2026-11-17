//! Handing part of a pull request to an agent from the detail: what is
//! sent (failing checks, open threads, selected lines), where (a writable agent chat
//! in the linked worktree, chosen among them), and the prompt itself,
//! prepared and shown before anything reaches a chat. Inserted into the
//! chat's input unsent; the chat is checked again then.

use daruda_github::GhError;
use daruda_store::project::LaneRef;
use gpui::{Context, Window};

use super::diff_layout::{LineRange, Side};
use super::read_slot::ReadTicket;
use super::{PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::github::PrLink;
use crate::workspace::Workspace;
use crate::workspace::github_ops::review_send::{PrSendTarget, checks_prompt, threads_prompt};
use crate::workspace::main_area::pane_tree::PaneId;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// What is handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum Payload {
    /// The failing checks, with their logs.
    FailedChecks,
    /// The unresolved review threads.
    OpenThreads,
    /// These lines selected in the Files tab, quoted as they were then.
    SelectedCode(LineRange),
}

/// What is read through `gh` to prepare a handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Read {
    FailedChecks,
    OpenThreads,
}

impl From<Read> for Payload {
    fn from(read: Read) -> Self {
        match read {
            Read::FailedChecks => Self::FailedChecks,
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

/// A handoff being prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Handoff {
    pub payload: Payload,
    /// The worktree whose chats can take it, and its name then.
    pub lane: LaneRef,
    pub lane_name: String,
    /// The chat chosen; `None` while the worktree has none.
    pub chat: Option<PaneId>,
    pub prompt: Prepared,
}

impl PrDetailState {
    pub(in crate::workspace) fn handoff(&self) -> Option<&Handoff> {
        self.handoff.as_ref()
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
        // Preparing again for what is already shown answers the click.
        if self
            .pr_detail_mut(id)
            .and_then(|detail| detail.handoff.as_ref())
            .is_some_and(|held| {
                held.payload == payload && matches!(held.prompt, Prepared::Reading(_))
            })
        {
            return;
        }
        let Some(ticket) = self.pr_detail_mut(id).map(|detail| detail.tickets.issue()) else {
            return;
        };
        let Some(target) = self.begin_pr_handoff(id, payload, Prepared::Reading(ticket), cx) else {
            return;
        };
        let link = target.link;
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || build(read, gh.as_ref(), &link),
            move |ws, built, cx| ws.land_pr_handoff(id, ticket, built, cx),
        )
        .detach();
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
        let Some(pr) = detail.pr.current().map(|pr| pr.summary.clone()) else {
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
    ) -> Option<PrSendTarget> {
        let target = self.pr_detail_send_target(id)?;
        let lane_name = self
            .pr_detail_mut(id)
            .map(|detail| detail.item.clone())
            .and_then(|item| self.pr_detail_lane(&item))
            .map(|lane| lane.name)
            .unwrap_or_default();
        let chat = self
            .writable_chats(target.lane, cx)
            .first()
            .map(|(pane, _, _)| *pane);
        let detail = self.pr_detail_mut(id)?;
        detail.handoff = Some(Handoff {
            payload,
            lane: target.lane,
            lane_name,
            chat,
            prompt,
        });
        cx.notify();
        Some(target)
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

    /// Put the prepared prompt into the chosen chat's input, unsent. The
    /// worktree must still be the one linked to the pull request, and the
    /// chat still a writable agent chat in it.
    pub(in crate::workspace) fn insert_pr_handoff(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handoff) = self.pr_detail_mut(id).and_then(|d| d.handoff.clone()) else {
            return;
        };
        let Prepared::Ready(prompt) = handoff.prompt else {
            return;
        };
        if self.pr_detail_send_target(id).map(|target| target.lane) != Some(handoff.lane) {
            self.report_pr_handoff_moved(cx);
            return;
        }
        if !self.pr_handoff_current(id, &handoff.payload, cx) {
            return;
        }
        // None chosen while the worktree had no chat: the one a send would
        // pick now. A chosen chat gone is never swapped for another.
        let Some(chat) = handoff.chat.or_else(|| {
            self.writable_chats(handoff.lane, cx)
                .first()
                .map(|(pane, _, _)| *pane)
        }) else {
            return;
        };
        if self.deliver_to_chat(handoff.lane, chat, prompt, window, cx)
            && let Some(detail) = self.pr_detail_mut(id)
        {
            detail.handoff = None;
        }
        cx.notify();
    }

    /// The worktree has no chat to take the prompt: open one there, with
    /// the agent a new chat would get, and put the prompt into it unsent.
    /// Works for a worktree never shown in this window.
    pub(in crate::workspace) fn insert_pr_handoff_in_new_chat(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(handoff) = self.pr_detail_mut(id).and_then(|d| d.handoff.clone()) else {
            return;
        };
        let Prepared::Ready(prompt) = handoff.prompt else {
            return;
        };
        if self.pr_detail_send_target(id).map(|target| target.lane) != Some(handoff.lane) {
            self.report_pr_handoff_moved(cx);
            return;
        }
        if !self.pr_handoff_current(id, &handoff.payload, cx) {
            return;
        }
        // A chat appeared meanwhile: the user picks among them instead.
        if !self.writable_chats(handoff.lane, cx).is_empty() {
            cx.notify();
            return;
        }
        // Checked on disk before anything is shown: a worktree whose folder
        // is gone would otherwise be activated into its empty state.
        self.recompute_availability_for(handoff.lane);
        let present = self.lane_for(handoff.lane).is_some_and(|lane| {
            lane.availability == crate::lane::availability::LaneAvailability::Present
        });
        if !present {
            self.report_pr_handoff_unavailable(cx);
            return;
        }
        self.activate_lane(handoff.lane, window, cx);
        let agent = self.open_agent_id(None);
        let cwds = self.active_lane_cwds();
        let Some(chat) = self.insert_agent_chat_pane(
            agent,
            cwds,
            daruda_config::ModeChoice::FollowDefault,
            window,
            cx,
        ) else {
            self.report_pr_handoff_unavailable(cx);
            return;
        };
        self.reveal_new_agent_chat_pane(chat, window, cx);
        if self.deliver_to_chat(handoff.lane, chat, prompt, window, cx)
            && let Some(detail) = self.pr_detail_mut(id)
        {
            detail.handoff = None;
        }
        cx.notify();
    }

    /// Whether what the handoff quotes is still what is selected: lines
    /// selected otherwise since, or a listing read at another head, close
    /// it with a warning rather than insert lines no longer meant.
    fn pr_handoff_current(
        &mut self,
        id: PrDetailId,
        payload: &Payload,
        cx: &mut Context<Self>,
    ) -> bool {
        let Payload::SelectedCode(range) = payload else {
            return true;
        };
        let Some(detail) = self.pr_detail_mut(id) else {
            return false;
        };
        if detail
            .files
            .as_ref()
            .is_some_and(|files| files.selection_current(range))
        {
            return true;
        }
        detail.handoff = None;
        let report = daruda_store::observability::error_report::ErrorReport::new(
            crate::surface::strings::pull_requests::handoff_selection_changed(),
        )
        .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
        .at(file!(), line!())
        .dedup("github.handoff.selection")
        .build();
        self.report_error(report, cx);
        cx.notify();
        false
    }

    fn report_pr_handoff_unavailable(&mut self, cx: &mut Context<Self>) {
        let report = daruda_store::observability::error_report::ErrorReport::new(
            crate::surface::strings::pull_requests::handoff_lane_unavailable(),
        )
        .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
        .at(file!(), line!())
        .dedup("github.handoff.unavailable")
        .build();
        self.report_error(report, cx);
    }

    fn report_pr_handoff_moved(&mut self, cx: &mut Context<Self>) {
        let report = daruda_store::observability::error_report::ErrorReport::new(
            crate::surface::strings::pull_requests::handoff_lane_moved(),
        )
        .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
        .at(file!(), line!())
        .dedup("github.handoff.moved")
        .build();
        self.report_error(report, cx);
    }
}

fn build(read: Read, gh: &dyn daruda_github::Gh, link: &PrLink) -> Result<Option<String>, GhError> {
    match read {
        Read::FailedChecks => checks_prompt(gh, link),
        Read::OpenThreads => threads_prompt(gh, link),
    }
}
