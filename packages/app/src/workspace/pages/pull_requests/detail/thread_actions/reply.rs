//! Replying to a review thread: the box and its draft, posting, and
//! holding a reply whose outcome is unknown until a threads read settles it.

use super::*;

impl Workspace {
    /// Open a reply box under `thread`, with the keyboard in it.
    pub(in crate::workspace) fn open_pr_reply(
        &mut self,
        id: PrDetailId,
        thread: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .pr_detail_mut(id)
            .and_then(|detail| detail.thread_work.work.get(thread))
            .is_some_and(|work| work.reply.is_some())
        {
            return;
        }
        // A draft kept from before continues where it was left.
        let owner = self.pr_draft_owner(id, cx);
        let draft = owner
            .as_ref()
            .and_then(|owner| DraftStore::of(owner, cx))
            .and_then(|drafts| {
                drafts
                    .replies
                    .into_iter()
                    .find(|held| held.thread == thread)
                    .map(|held| held.text)
            })
            .unwrap_or_default();
        let reply = crate::ui::make_markdown_prose_state(
            &draft,
            s::reply_placeholder(),
            crate::ui::theme::PR_REPLY_ROWS,
            window,
            cx,
        );
        let typed = thread.to_owned();
        let typing = cx.subscribe(&reply, move |ws, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                ws.pr_reply_typed(id, &typed, cx);
            }
        });
        reply.update(cx, |reply, cx| reply.focus(window, cx));
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let work = detail
            .thread_work
            .work
            .entry(thread.to_owned())
            .or_default();
        work.reply = Some(reply);
        work.typing = Some(typing);
        work.owner = owner.clone();
        // Sent before, from another detail or before a restart, and never
        // confirmed: it is checked again rather than offered for Post.
        if work.op.is_none()
            && let Some(sent) = owner.and_then(|owner| DraftStore::reply_sent(&owner, thread, cx))
        {
            self.hold_pr_reply(id, thread, sent, cx);
        }
        cx.notify();
    }

    /// Hold `thread`'s reply, sent and not confirmed, until a threads read
    /// sent now says whether it posted.
    fn hold_pr_reply(
        &mut self,
        id: PrDetailId,
        thread: &str,
        sent: SentReply,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (after, read) = detail.reread_threads();
        detail
            .thread_work
            .work
            .entry(thread.to_owned())
            .or_default()
            .op = Some(ThreadOp::Checking(PendingReply { after, sent }));
        if let Some(read) = read {
            self.send_pr_tab_read(id, read, cx);
        }
        cx.notify();
    }

    /// Close `thread`'s reply box, dropping what was written.
    pub(in crate::workspace) fn cancel_pr_reply(
        &mut self,
        id: PrDetailId,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if detail.thread_work.running(thread) {
            return;
        }
        if let Some(work) = detail.thread_work.work.get_mut(thread) {
            work.close_reply();
            work.op = None;
        }
        self.set_pr_reply_draft(id, thread, None, cx);
        cx.notify();
    }

    /// The person looked on GitHub: the reply that could not be confirmed
    /// did not post, so its text may go again.
    pub(in crate::workspace) fn mark_pr_reply_not_posted(
        &mut self,
        id: PrDetailId,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(work) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.thread_work.work.get_mut(thread))
            && work.not_posted()
        {
            self.let_go_pr_reply_sent(id, thread, cx);
            cx.notify();
        }
    }

    /// A reply held as sent turned out not posted: its draft is only text.
    pub(in crate::workspace) fn let_go_pr_reply_sent(
        &mut self,
        id: PrDetailId,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(owner) = self.pr_draft_owner(id, cx) {
            DraftStore::set_reply_sent(&owner, thread, None, cx);
        }
    }

    /// Typing a reply: kept as typed, so leaving or quitting loses none.
    fn pr_reply_typed(&mut self, id: PrDetailId, thread: &str, cx: &mut Context<Self>) {
        let Some(work) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.thread_work.work.get_mut(thread))
        else {
            return;
        };
        if work.sent_unsettled() {
            work.typed_after_send = true;
        }
        let text = work
            .reply
            .as_ref()
            .map(|reply| reply.read(cx).value().to_string())
            .filter(|text| !text.trim().is_empty());
        if let Some(owner) = work.owner.clone() {
            DraftStore::set_reply(&owner, thread, text, cx);
        }
        cx.notify();
    }

    /// Drop the reply draft kept for `thread`, which is no longer listed.
    pub(in crate::workspace) fn discard_pr_reply_draft(
        &mut self,
        id: PrDetailId,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        self.set_pr_reply_draft(id, thread, None, cx);
        cx.notify();
    }

    pub(in crate::workspace) fn set_pr_reply_draft(
        &mut self,
        id: PrDetailId,
        thread: &str,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(owner) = self.pr_draft_owner(id, cx) {
            DraftStore::set_reply(&owner, thread, text, cx);
        }
    }

    /// Post what `thread`'s reply box holds.
    pub(in crate::workspace) fn post_pr_reply(
        &mut self,
        id: PrDetailId,
        thread: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(body) = self
            .pr_detail_mut(id)
            .and_then(|detail| detail.thread_work.work.get(thread))
            .and_then(|work| work.reply.as_ref())
            .map(|reply| reply.read(cx).value().to_string())
            .filter(|body| !body.trim().is_empty())
        else {
            return;
        };
        // Held as sent elsewhere (another detail, before a restart) and not
        // confirmed: checked, not sent again.
        if let Some(sent) = self
            .pr_draft_owner(id, cx)
            .and_then(|owner| DraftStore::reply_sent(&owner, thread, cx))
        {
            let held = self
                .pr_detail_mut(id)
                .and_then(|detail| detail.thread_work.work.get(thread))
                .is_some_and(|work| {
                    matches!(
                        work.op,
                        Some(ThreadOp::Checking(_) | ThreadOp::Unconfirmed(_))
                    )
                });
            if !held {
                self.hold_pr_reply(id, thread, sent, cx);
            }
            return;
        }
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if !detail.thread_work.begin(thread, ThreadAction::Replying) {
            return;
        }
        let shown = match detail.threads.as_ref().map(|slot| slot.load()) {
            Some(Load::Ready(threads)) => threads.threads.iter().find(|held| held.id == thread),
            _ => None,
        };
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        // The account gh posts as now; the box's own if that is not known.
        let by = DraftStore::owner(&repo, number, cx)
            .or_else(|| {
                detail
                    .thread_work
                    .work
                    .get(thread)
                    .and_then(|work| work.owner.clone())
            })
            .map(|owner| owner.account);
        let outgoing = Outgoing::to(shown, body.clone(), by);
        let (gh, thread) = (GhHost::global(cx), thread.to_owned());
        let bg = (repo.clone(), thread.clone());
        spawn_bg_work_and_mutate(
            cx,
            move || gh.reply_to_thread(&bg.0, &bg.1, &body),
            move |ws, result, cx| {
                ws.land_pr_reply(id, (repo, number), thread, outgoing, result, cx)
            },
        )
        .detach();
        cx.notify();
    }

    fn land_pr_reply(
        &mut self,
        id: PrDetailId,
        (repo, number): (RepoId, u64),
        thread: String,
        outgoing: Outgoing,
        result: Result<ReviewComment, GhError>,
        cx: &mut Context<Self>,
    ) {
        let failure = result.as_ref().err().map(ToString::to_string);
        // Even if the detail is gone: failed, it is held as sent, so no
        // detail sends it again unchecked.
        let sent = result.as_ref().err().map(|e| outgoing.failed(e));
        let owner = DraftStore::owner(&repo, number, cx);
        if let (Some(owner), Some(sent)) = (&owner, &sent) {
            DraftStore::set_reply_sent(owner, &thread, Some(sent.clone()), cx);
        }
        if let Some(detail) = self.pr_detail_mut(id) {
            let stamp = detail.tickets.issue();
            let finished =
                detail
                    .thread_work
                    .finish(&thread, ThreadAction::Replying, failure, stamp);
            if finished
                && let Ok(comment) = &result
                && let Some(read) = detail.record_posted_reply(&thread, comment)
            {
                self.send_pr_tab_read(id, read, cx);
            }
        }
        // Posted, the draft is done with, unless typed into since: then it
        // holds what the box still shows.
        if sent.is_none()
            && let Some(owner) = &owner
        {
            let left = self.pr_reply_left(id, &thread, cx);
            DraftStore::set_reply(owner, &thread, left, cx);
        }
        let reread = self.thread_action_landed(id, repo, ThreadAction::Replying, result.err(), cx);
        // It may have posted all the same: the threads read again say.
        if let Some(after) = reread
            && let Some(sent) = sent
            && let Some(work) = self
                .pr_detail_mut(id)
                .and_then(|detail| detail.thread_work.work.get_mut(&thread))
            && matches!(work.op, Some(ThreadOp::Failed(ThreadAction::Replying, _)))
        {
            work.op = Some(ThreadOp::Checking(PendingReply { after, sent }));
        }
    }

    /// What `thread`'s reply box still holds, when it stayed open.
    pub(in crate::workspace) fn pr_reply_left(
        &self,
        id: PrDetailId,
        thread: &str,
        cx: &Context<Self>,
    ) -> Option<String> {
        self.pr_detail(id)?
            .thread_work
            .work
            .get(thread)?
            .reply
            .as_ref()
            .map(|reply| reply.read(cx).value().to_string())
            .filter(|text| !text.trim().is_empty())
    }
}
