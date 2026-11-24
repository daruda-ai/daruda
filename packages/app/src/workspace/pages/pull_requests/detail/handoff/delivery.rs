//! Insert only after checking the worktree, chat and selected code again.

use super::*;
impl Workspace {
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
        let Some(prompt) = handoff.draft(cx) else {
            return;
        };
        let Some(lane) = self.pr_handoff_lane(id, cx) else {
            return;
        };
        if !self.pr_handoff_current(id, &handoff.payload, cx) {
            return;
        }
        // None chosen while the worktree had no chat: the one a send would
        // pick now. A chosen chat gone is never swapped for another.
        let Some(chat) = handoff
            .chat
            .or_else(|| self.writable_chats(lane, cx).first().map(|chat| chat.pane))
        else {
            return;
        };
        if self.deliver_to_chat(lane, chat, prompt, window, cx)
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
        let Some(prompt) = handoff.draft(cx) else {
            return;
        };
        let Some(lane) = self.pr_handoff_lane(id, cx) else {
            return;
        };
        if !self.pr_handoff_current(id, &handoff.payload, cx) {
            return;
        }
        // A chat appeared meanwhile: the user picks among them instead.
        if !self.writable_chats(lane, cx).is_empty() {
            cx.notify();
            return;
        }
        self.insert_new_pr_chat(id, lane, prompt, window, cx);
    }

    pub(in crate::workspace) fn checkout_for_pr_handoff(
        &mut self,
        id: PrDetailId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail(id) else {
            return;
        };
        let Some(held) = detail.handoff.clone() else {
            return;
        };
        let Some(prompt) = held.draft(cx) else { return };
        let item = detail.item.clone();
        match self.pr_handoff_destination(id) {
            Some(Destination::Checkout) => {}
            // A worktree is linked now: the view offers its chats instead.
            Some(Destination::To(_)) | None => return,
            Some(Destination::Moved) => {
                self.report_pr_handoff_moved(cx);
                return;
            }
        }
        if self
            .pages
            .pull_requests
            .creating
            .contains_key(&detail.key())
            || !self.pr_handoff_current(id, &held.payload, cx)
        {
            return;
        }
        self.checkout_pr_for(
            item,
            None,
            Some(Box::new(move |ws, lane, window, cx| {
                let Some(current) = ws
                    .pr_detail_mut(id)
                    .and_then(|detail| detail.handoff.as_mut())
                else {
                    return;
                };
                if current.lane != HandoffLane::Unlinked {
                    return;
                }
                let deliver = current.id == held.id && current.draft(cx).as_ref() == Some(&prompt);
                current.lane = HandoffLane::Made(lane);
                cx.notify();
                if !deliver || !ws.pr_handoff_current(id, &held.payload, cx) {
                    return;
                }
                ws.insert_new_pr_chat(id, lane, prompt, window, cx);
            })),
            window,
            cx,
        );
    }

    /// The worktree an insert goes to, settling a handoff shown without one
    /// onto the worktree linked since. `None`, said when it moved, when it
    /// has none to go to.
    fn pr_handoff_lane(&mut self, id: PrDetailId, cx: &mut Context<Self>) -> Option<LaneRef> {
        self.settle_pr_handoff_lane(id);
        match self.pr_handoff_destination(id)? {
            Destination::To(lane) => Some(lane),
            Destination::Checkout => None,
            Destination::Moved => {
                self.report_pr_handoff_moved(cx);
                None
            }
        }
    }

    /// Take the worktree the pull request links as the handoff's own, once
    /// it is seen: one shown without a worktree goes there, and one made for
    /// it is from then on held to that link, so a later unlink is a move.
    pub(in super::super) fn settle_pr_handoff_lane(&mut self, id: PrDetailId) {
        let Some(detail) = self.pr_detail(id) else {
            return;
        };
        let Some(linked) = self.pr_detail_lane(&detail.item).map(|lane| lane.lane) else {
            return;
        };
        if let Some(held) = self.pr_detail_mut(id).and_then(|d| d.handoff.as_mut())
            && (held.lane == HandoffLane::Unlinked || held.lane == HandoffLane::Made(linked))
        {
            held.lane = HandoffLane::Linked(linked);
        }
    }

    fn insert_new_pr_chat(
        &mut self,
        id: PrDetailId,
        lane: LaneRef,
        prompt: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Checked on disk before anything is shown: a worktree whose folder
        // is gone would otherwise be activated into its empty state.
        self.recompute_availability_for(lane);
        let present = self.lane_for(lane).is_some_and(|lane| {
            lane.availability == crate::lane::availability::LaneAvailability::Present
        });
        if !present {
            self.report_pr_handoff_unavailable(cx);
            return;
        }
        let back = (self.active_ref(), self.active_page());
        self.activate_lane(lane, window, cx);
        // Only in that worktree: a chat opened wherever is shown instead
        // would take a prompt about another pull request.
        let chat = if self.active_ref() == lane {
            let agent = self.open_agent_id(None);
            let cwds = self.active_lane_cwds();
            self.insert_agent_chat_pane(
                agent,
                cwds,
                daruda_config::ModeChoice::FollowDefault,
                window,
                cx,
            )
        } else {
            None
        };
        let Some(chat) = chat else {
            // Nothing opened: back where the person asked from.
            self.activate_lane(back.0, window, cx);
            if let Some(page) = back.1 {
                self.show_page(page, cx);
            }
            self.report_pr_handoff_unavailable(cx);
            return;
        };
        self.reveal_new_agent_chat_pane(chat, window, cx);
        if self.deliver_to_chat(lane, chat, prompt, window, cx)
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

    #[track_caller]
    fn report_pr_handoff_unavailable(&mut self, cx: &mut Context<Self>) {
        let at = std::panic::Location::caller();
        let report = daruda_store::observability::error_report::ErrorReport::new(
            crate::surface::strings::pull_requests::handoff_lane_unavailable(),
        )
        .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
        .at(at.file(), at.line())
        .dedup("github.handoff.unavailable")
        .build();
        self.report_error(report, cx);
    }

    #[track_caller]
    fn report_pr_handoff_moved(&mut self, cx: &mut Context<Self>) {
        let at = std::panic::Location::caller();
        let report = daruda_store::observability::error_report::ErrorReport::new(
            crate::surface::strings::pull_requests::handoff_lane_moved(),
        )
        .severity(daruda_store::observability::error_report::ErrorSeverity::Warning)
        .at(at.file(), at.line())
        .dedup("github.handoff.moved")
        .build();
        self.report_error(report, cx);
    }
}

#[cfg(test)]
mod tests {}
