//! What a person does to a review thread from the detail: reply to it, and
//! resolve or unresolve it. Each thread's action runs on its own, so one
//! thread's failure never overwrites another's. A failed reply keeps its
//! text, and every failure reads the threads again: `gh` failing does not
//! prove GitHub did nothing, so a reply that may have posted cannot be
//! posted again until a read says whether it did, or, when no read can, the
//! person says it did not.

use std::collections::HashMap;

use daruda_github::{GhError, RepoId, ReviewComment, ReviewThread, ReviewThreads};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{Context, Entity, Subscription, Window};

use super::read_slot::ReadTicket;
use super::{Load, PrDetailId, PrDetailState, TabRead};
use crate::gh_host::GhHost;
use crate::github::{DraftStore, PrPoller};
use crate::surface::strings::pull_requests as s;
use crate::ui::{InputEvent, TextareaState};
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;
use daruda_store::review_drafts::{DraftOwner, SentReply};

/// An action on one thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum ThreadAction {
    Replying,
    Resolving,
    Unresolving,
}

impl ThreadAction {
    pub(in crate::workspace) fn failure_title(self) -> String {
        match self {
            Self::Replying => s::thread_reply_failed(),
            Self::Resolving => s::thread_resolve_failed(),
            Self::Unresolving => s::thread_unresolve_failed(),
        }
    }
}

/// Where a thread's last action stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ThreadOp {
    Running(ThreadAction),
    /// What failed and what `gh` said, until the next action on it.
    Failed(ThreadAction, String),
    /// A reply that failed may still have posted: the threads read under
    /// its ticket or a later one say whether it did. Post waits.
    Checking(PendingReply),
    /// No read under its ticket or later could say whether it posted (the
    /// read failed, or could not tell): Post and other actions on the
    /// thread wait for one that can, or for the person to say it did not.
    Unconfirmed(PendingReply),
}

/// A reply being checked: the first threads read that can settle it, and
/// what was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct PendingReply {
    pub after: ReadTicket,
    pub sent: SentReply,
}

/// A reply going out: its text, who sends it, and the thread's last comment
/// as it goes. Only a comment after that one can be it, so an earlier comment
/// with the same text is never taken for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Outgoing {
    body: String,
    /// `None` when the thread's end was not read (more replies than are
    /// read, or not read yet), so no read can tell.
    after: Option<String>,
    /// The account it goes out as; `None` when not known, and then any
    /// author's comment with its text counts.
    by: Option<String>,
}

impl Outgoing {
    /// What `by` sends to `thread` as shown.
    pub(in crate::workspace) fn to(
        thread: Option<&ReviewThread>,
        body: String,
        by: Option<String>,
    ) -> Self {
        let after = thread
            .filter(|thread| !thread.more_comments)
            .and_then(|thread| thread.comments.last())
            .map(|comment| comment.id.clone());
        Self { body, after, by }
    }

    /// It failed with `error`, which does not prove it did not post. A
    /// request cut off waiting may still post: our own timeout, or GitHub
    /// ending one past its 10 seconds with a 502 or 504, applied or not.
    pub(in crate::workspace) fn failed(self, error: &GhError) -> SentReply {
        let cut_off = match error {
            GhError::Timeout(_) => true,
            GhError::Exit { stderr, .. } => {
                CUT_OFF_BY_GITHUB.iter().any(|said| stderr.contains(said))
            }
            _ => false,
        };
        SentReply {
            body: self.body,
            after: self.after,
            by: self.by,
            reason: error.to_string(),
            cut_off,
        }
    }
}

/// What `gh` prints when GitHub ended the request unanswered: `gh: HTTP 502`
/// for a body that is not JSON, `gh: <message> (HTTP 504)` for one that is.
const CUT_OFF_BY_GITHUB: [&str; 3] = [
    "HTTP 502",
    "HTTP 504",
    "couldn't respond to your request in time",
];

/// Whether `sent` is in `thread` as read; `None` when that read cannot say:
/// the thread is not listed (resolved since, or gone), its end is not read,
/// the comment it came after is no longer there, or it is missing but was
/// cut off, so GitHub may post it yet.
pub(in crate::workspace) fn arrived(
    sent: &SentReply,
    thread: Option<&ReviewThread>,
) -> Option<bool> {
    let thread = thread.filter(|thread| !thread.more_comments)?;
    let after = sent.after.as_deref()?;
    let start = thread
        .comments
        .iter()
        .position(|comment| comment.id == after)?
        + 1;
    let body = reply_text(&sent.body);
    let found = thread.comments[start..].iter().any(|comment| {
        reply_text(&comment.body) == body
            && sent
                .by
                .as_deref()
                .is_none_or(|by| comment.author.as_deref() == Some(by))
    });
    match (found, sent.cut_off) {
        (true, _) => Some(true),
        (false, true) => None,
        (false, false) => Some(false),
    }
}

/// A reply's text as compared: GitHub may give it back with other line
/// endings or without trailing spaces, which do not change it.
fn reply_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// How a reply being checked turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum Settled {
    Posted,
    NotPosted,
}

/// One thread's reply box and action.
#[derive(Default)]
pub(in crate::workspace) struct ThreadWork {
    /// Open while a reply is written; kept when posting it fails.
    pub reply: Option<Entity<TextareaState>>,
    /// Redraws the detail as the reply is typed, so Post follows it.
    typing: Option<Subscription>,
    pub op: Option<ThreadOp>,
    /// Whose draft the reply is, fixed when the box opened.
    owner: Option<DraftOwner>,
}

impl ThreadWork {
    /// A reply still being checked when its threads answer at `ticket`:
    /// `arrived` says whether it posted, or `None` when that read cannot.
    /// Posted, the box closes; not posted, it failed after all; unknown, it
    /// stays unconfirmed.
    fn checked(
        &mut self,
        ticket: ReadTicket,
        arrived: impl FnOnce(&SentReply) -> Option<bool>,
    ) -> Option<Settled> {
        let Some(ThreadOp::Checking(pending) | ThreadOp::Unconfirmed(pending)) = &self.op else {
            return None;
        };
        if ticket < pending.after {
            return None;
        }
        let pending = pending.clone();
        match arrived(&pending.sent) {
            Some(true) => {
                self.close_reply();
                self.op = None;
                Some(Settled::Posted)
            }
            Some(false) => {
                self.op = Some(ThreadOp::Failed(
                    ThreadAction::Replying,
                    pending.sent.reason,
                ));
                Some(Settled::NotPosted)
            }
            None => {
                self.op = Some(ThreadOp::Unconfirmed(pending));
                None
            }
        }
    }

    /// The person says a reply that could not be confirmed did not post:
    /// it failed, and its text can go again. `false` unless unconfirmed.
    fn not_posted(&mut self) -> bool {
        let Some(ThreadOp::Unconfirmed(pending)) = &self.op else {
            return false;
        };
        self.op = Some(ThreadOp::Failed(
            ThreadAction::Replying,
            pending.sent.reason.clone(),
        ));
        true
    }

    fn close_reply(&mut self) {
        self.reply = None;
        self.typing = None;
    }
}

impl std::fmt::Debug for ThreadWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadWork")
            .field("reply", &self.reply.as_ref().map(Entity::entity_id))
            .field("op", &self.op)
            .finish()
    }
}

/// Every thread's work in one detail, and the threads resolved from it,
/// each with a ticket issued when its resolve landed: a read sent later no
/// longer lists it.
#[derive(Debug, Default)]
pub(in crate::workspace) struct ThreadsWork {
    work: HashMap<String, ThreadWork>,
    resolved: HashMap<String, ReadTicket>,
}

impl ThreadsWork {
    pub(in crate::workspace) fn get(&self, thread: &str) -> Option<&ThreadWork> {
        self.work.get(thread)
    }

    pub(in crate::workspace) fn resolved_here(&self, thread: &str) -> bool {
        self.resolved.contains_key(thread)
    }

    fn running(&self, thread: &str) -> bool {
        matches!(
            self.work.get(thread).and_then(|work| work.op.as_ref()),
            Some(ThreadOp::Running(_))
        )
    }

    /// Start `action` on `thread`, unless one runs there already or a
    /// failed reply's outcome is still being checked.
    fn begin(&mut self, thread: &str, action: ThreadAction) -> bool {
        let work = self.work.entry(thread.to_owned()).or_default();
        if matches!(
            work.op,
            Some(ThreadOp::Running(_) | ThreadOp::Checking(_) | ThreadOp::Unconfirmed(_))
        ) {
            return false;
        }
        work.op = Some(ThreadOp::Running(action));
        true
    }

    /// `action` on `thread` ended; `stamp` is a ticket issued as it landed.
    /// `true` if it was the one running there.
    fn finish(
        &mut self,
        thread: &str,
        action: ThreadAction,
        failure: Option<String>,
        stamp: ReadTicket,
    ) -> bool {
        let Some(work) = self.work.get_mut(thread) else {
            return false;
        };
        if work.op != Some(ThreadOp::Running(action)) {
            return false;
        }
        work.op = failure.map(|reason| ThreadOp::Failed(action, reason));
        match (action, &work.op) {
            (ThreadAction::Replying, None) => work.close_reply(),
            (ThreadAction::Resolving, None) => {
                self.resolved.insert(thread.to_owned(), stamp);
            }
            (ThreadAction::Unresolving, None) => {
                self.resolved.remove(thread);
            }
            _ => {}
        }
        true
    }

    /// The threads read under `ticket` landed, as `threads` when read.
    /// Resolved marks older than a read that arrived go (it no longer lists
    /// them); a failed reply it may have posted is looked for after the
    /// comment it followed, and stays unconfirmed when the read failed or
    /// cannot tell; work on threads gone is let go, unless it holds a reply
    /// still being written.
    /// Answers the threads whose failed reply this read settled, and how.
    pub(in crate::workspace) fn threads_read(
        &mut self,
        ticket: ReadTicket,
        threads: Option<&ReviewThreads>,
    ) -> Vec<(String, Settled)> {
        // Unread, the threads shown are the older ones, which still need the
        // marks.
        let Some(threads) = threads else {
            for work in self.work.values_mut() {
                work.checked(ticket, |_| None);
            }
            return Vec::new();
        };
        self.resolved.retain(|_, stamp| *stamp > ticket);
        let mut settled = Vec::new();
        for (thread, work) in &mut self.work {
            let held = threads.threads.iter().find(|held| held.id == *thread);
            if let Some(how) = work.checked(ticket, |sent| arrived(sent, held)) {
                settled.push((thread.clone(), how));
            }
        }
        self.work.retain(|thread, work| {
            work.reply.is_some()
                || matches!(work.op, Some(ThreadOp::Running(_)))
                || threads.threads.iter().any(|held| held.id == *thread)
        });
        settled
    }
}

impl PrDetailState {
    /// Read the threads again in place. The ticket is issued even when they
    /// are not held, so a check waiting on it settles with whichever threads
    /// read comes next.
    pub(super) fn reread_threads(&mut self) -> (ReadTicket, Option<TabRead>) {
        let ticket = self.tickets.issue();
        let read = self.threads.as_mut().map(|threads| {
            threads.reread(ticket);
            TabRead::Threads(ticket)
        });
        (ticket, read)
    }
}

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
    pub(super) fn let_go_pr_reply_sent(
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
            .and_then(|detail| detail.thread_work.work.get(thread))
        else {
            return;
        };
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

    pub(super) fn set_pr_reply_draft(
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
        // Even if the detail is gone: posted, the draft is done with; failed,
        // it is held as sent, so no detail sends it again unchecked.
        let sent = result.as_ref().err().map(|e| outgoing.failed(e));
        if let Some(owner) = DraftStore::owner(&repo, number, cx) {
            match &sent {
                None => DraftStore::set_reply(&owner, &thread, None, cx),
                Some(sent) => DraftStore::set_reply_sent(&owner, &thread, Some(sent.clone()), cx),
            }
        }
        if let Some(detail) = self.pr_detail_mut(id) {
            let stamp = detail.tickets.issue();
            let finished =
                detail
                    .thread_work
                    .finish(&thread, ThreadAction::Replying, failure, stamp);
            if finished
                && let Ok(comment) = &result
                && let Some(threads) = detail.threads.as_mut().and_then(|slot| slot.ready_mut())
                && let Some(held) = threads.threads.iter_mut().find(|held| held.id == thread)
            {
                held.comments.push(comment.clone());
            }
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

    /// Resolve `thread`, or unresolve one resolved from here.
    pub(in crate::workspace) fn set_pr_thread_resolved(
        &mut self,
        id: PrDetailId,
        thread: &str,
        resolved: bool,
        cx: &mut Context<Self>,
    ) {
        let action = if resolved {
            ThreadAction::Resolving
        } else {
            ThreadAction::Unresolving
        };
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if !detail.thread_work.begin(thread, action) {
            return;
        }
        let repo = detail.item.repo.clone();
        let (gh, thread) = (GhHost::global(cx), thread.to_owned());
        let bg = (repo.clone(), thread.clone());
        spawn_bg_work_and_mutate(
            cx,
            move || gh.set_thread_resolved(&bg.0, &bg.1, resolved),
            move |ws, result, cx| {
                let failure = result.as_ref().err().map(ToString::to_string);
                if let Some(detail) = ws.pr_detail_mut(id) {
                    let stamp = detail.tickets.issue();
                    detail.thread_work.finish(&thread, action, failure, stamp);
                }
                ws.thread_action_landed(id, repo, action, result.err(), cx);
            },
        )
        .detach();
        cx.notify();
    }

    /// GitHub changed, or may have, whether or not the detail that asked is
    /// still shown: the list and lane badges are asked again and a failure
    /// is reported. The detail, if still there, reads the threads again
    /// after a failure: the ticket of that read.
    fn thread_action_landed(
        &mut self,
        id: PrDetailId,
        repo: RepoId,
        action: ThreadAction,
        error: Option<GhError>,
        cx: &mut Context<Self>,
    ) -> Option<ReadTicket> {
        PrPoller::nudge(repo, cx);
        if let Some(e) = error {
            let report = ErrorReport::new(action.failure_title())
                .severity(ErrorSeverity::Error)
                .from_error(&e)
                .at(file!(), line!())
                .dedup("github.pr_thread.action")
                .build();
            self.report_error(report, cx);
            let reread = self.pr_detail_mut(id).map(|detail| detail.reread_threads());
            if let Some((_, Some(read))) = &reread {
                self.send_pr_tab_read(id, *read, cx);
            }
            cx.notify();
            return reread.map(|(ticket, _)| ticket);
        }
        cx.notify();
        None
    }
}

#[cfg(test)]
mod tests;
