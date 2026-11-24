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
    /// Shared by the navigation button in this thread's two locations.
    pub(super) navigation_focus: Option<gpui::FocusHandle>,
    /// Redraws the detail as the reply is typed, so Post follows it.
    typing: Option<Subscription>,
    pub op: Option<ThreadOp>,
    /// Whose draft the reply is, fixed when the box opened.
    owner: Option<DraftOwner>,
    /// Typed into the box after its text went out: posting it then leaves
    /// the box open with what was typed, rather than closing it unread.
    typed_after_send: bool,
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
                self.posted();
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
        self.typed_after_send = false;
    }

    /// The reply went out: its box closes unless typed into since.
    fn posted(&mut self) {
        if std::mem::take(&mut self.typed_after_send) {
            return;
        }
        self.close_reply();
    }

    /// Whether the box's text has gone out and is not settled yet.
    fn sent_unsettled(&self) -> bool {
        matches!(
            self.op,
            Some(
                ThreadOp::Running(ThreadAction::Replying)
                    | ThreadOp::Checking(_)
                    | ThreadOp::Unconfirmed(_)
            )
        )
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
    /// A tab jump keeps keyboard input with this thread, including while
    /// the other tab is still waiting for its file listing.
    pub(super) fn focus_after_jump(
        &mut self,
        thread: &str,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let held = self.work.entry(thread.to_owned()).or_default();
        if let Some(reply) = &held.reply {
            reply.update(cx, |reply, cx| reply.focus(window, cx));
        } else {
            held.navigation_focus
                .get_or_insert_with(|| cx.focus_handle())
                .focus(window, cx);
        }
    }

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
        if action == ThreadAction::Replying {
            work.typed_after_send = false;
        }
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
            (ThreadAction::Replying, None) => work.posted(),
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
    /// A refresh may already include this reply or still be returning a snapshot
    /// from before it posted. Keep exactly one copy and supersede the older read.
    fn record_posted_reply(&mut self, thread: &str, comment: &ReviewComment) -> Option<TabRead> {
        let slot = self.threads.as_mut()?;
        if let Some(held) = slot
            .ready_mut()
            .and_then(|read| read.threads.iter_mut().find(|held| held.id == thread))
            && !held.comments.iter().any(|held| held.id == comment.id)
        {
            held.comments.push(comment.clone());
        }
        slot.waiting()?;
        self.reread_threads().1
    }

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

mod reply;

#[cfg(test)]
mod tests;
