//! What a person does to a review thread from the detail: reply to it, and
//! resolve or unresolve it. Each thread's action runs on its own, so one
//! thread's failure never overwrites another's. A failed reply keeps its
//! text, and every failure reads the threads again: `gh` failing does not
//! prove GitHub did nothing, so a reply that may have posted cannot be
//! posted again until that read says whether it did.

use std::collections::HashMap;

use daruda_github::{GhError, RepoId, ReviewComment, ReviewThreads};
use daruda_store::observability::error_report::{ErrorReport, ErrorSeverity};
use gpui::{Context, Entity, Subscription, Window};

use super::read_slot::ReadTicket;
use super::{PrDetailId, PrDetailState, TabRead};
use crate::gh_host::GhHost;
use crate::github::PrPoller;
use crate::surface::strings::pull_requests as s;
use crate::ui::{InputEvent, TextareaState};
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

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
}

/// One thread's reply box and action.
#[derive(Default)]
pub(in crate::workspace) struct ThreadWork {
    /// Open while a reply is written; kept when posting it fails.
    pub reply: Option<Entity<TextareaState>>,
    /// Redraws the detail as the reply is typed, so Post follows it.
    typing: Option<Subscription>,
    pub op: Option<ThreadOp>,
    /// A reply that failed may still have posted: the threads read under
    /// this ticket, or a later one, say whether it did. Post waits.
    checking: Option<ReadTicket>,
}

impl ThreadWork {
    pub(in crate::workspace) fn checking(&self) -> bool {
        self.checking.is_some()
    }

    fn close_reply(&mut self) {
        self.reply = None;
        self.typing = None;
        self.checking = None;
    }
}

impl std::fmt::Debug for ThreadWork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadWork")
            .field("reply", &self.reply.as_ref().map(Entity::entity_id))
            .field("op", &self.op)
            .field("checking", &self.checking)
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
        if matches!(work.op, Some(ThreadOp::Running(_))) || work.checking.is_some() {
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
    /// Resolved marks older than the read go (it no longer lists them); a
    /// failed reply it may have posted is checked against the thread's last
    /// comment (`draft` reads its box); work on threads gone is let go,
    /// unless it holds a reply still being written.
    pub(in crate::workspace) fn threads_read(
        &mut self,
        ticket: ReadTicket,
        threads: Option<&ReviewThreads>,
        draft: impl Fn(&Entity<TextareaState>) -> String,
    ) {
        self.resolved.retain(|_, stamp| *stamp > ticket);
        let Some(threads) = threads else {
            for work in self.work.values_mut() {
                if work.checking.is_some_and(|asked| ticket >= asked) {
                    work.checking = None;
                }
            }
            return;
        };
        let last = |thread: &str| {
            threads
                .threads
                .iter()
                .find(|held| held.id == thread)
                .and_then(|held| held.comments.last())
                .map(|comment| comment.body.trim().to_owned())
        };
        for (thread, work) in &mut self.work {
            if !work.checking.is_some_and(|asked| ticket >= asked) {
                continue;
            }
            work.checking = None;
            let posted = work
                .reply
                .as_ref()
                .is_some_and(|reply| last(thread).as_deref() == Some(draft(reply).trim()));
            if posted {
                work.close_reply();
                work.op = None;
            }
        }
        self.work.retain(|thread, work| {
            work.reply.is_some()
                || matches!(work.op, Some(ThreadOp::Running(_)))
                || threads.threads.iter().any(|held| held.id == *thread)
        });
    }
}

impl PrDetailState {
    /// Read the threads again in place, keeping what is shown until then.
    fn reread_threads(&mut self) -> Option<(ReadTicket, TabRead)> {
        let ticket = self.tickets.issue();
        self.threads.as_mut()?.reread(ticket);
        Some((ticket, TabRead::Threads(ticket)))
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
        let reply = crate::ui::make_markdown_prose_state(
            "",
            s::reply_placeholder(),
            crate::ui::theme::PR_REPLY_ROWS,
            window,
            cx,
        );
        let typing = cx.subscribe(&reply, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
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
        cx.notify();
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
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if !detail.thread_work.begin(thread, ThreadAction::Replying) {
            return;
        }
        let repo = detail.item.repo.clone();
        let (gh, thread) = (GhHost::global(cx), thread.to_owned());
        let bg = (repo.clone(), thread.clone());
        spawn_bg_work_and_mutate(
            cx,
            move || gh.reply_to_thread(&bg.0, &bg.1, &body),
            move |ws, result, cx| ws.land_pr_reply(id, repo, thread, result, cx),
        )
        .detach();
        cx.notify();
    }

    fn land_pr_reply(
        &mut self,
        id: PrDetailId,
        repo: RepoId,
        thread: String,
        result: Result<ReviewComment, GhError>,
        cx: &mut Context<Self>,
    ) {
        let failure = result.as_ref().err().map(ToString::to_string);
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
        self.thread_action_landed(id, repo, &thread, ThreadAction::Replying, result.err(), cx);
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
                ws.thread_action_landed(id, repo, &thread, action, result.err(), cx);
            },
        )
        .detach();
        cx.notify();
    }

    /// GitHub changed, or may have, whether or not the detail that asked is
    /// still shown: the list and lane badges are asked again and a failure
    /// is reported. The detail, if still there, reads the threads again; a
    /// failed reply waits on that read before it can be posted again.
    fn thread_action_landed(
        &mut self,
        id: PrDetailId,
        repo: RepoId,
        thread: &str,
        action: ThreadAction,
        error: Option<GhError>,
        cx: &mut Context<Self>,
    ) {
        PrPoller::nudge(repo, cx);
        if let Some(e) = error {
            let report = ErrorReport::new(action.failure_title())
                .severity(ErrorSeverity::Error)
                .from_error(&e)
                .at(file!(), line!())
                .dedup("github.pr_thread.action")
                .build();
            self.report_error(report, cx);
            let reread = self.pr_detail_mut(id).and_then(|detail| {
                let (ticket, read) = detail.reread_threads()?;
                if action == ThreadAction::Replying
                    && let Some(work) = detail.thread_work.work.get_mut(thread)
                {
                    work.checking = Some(ticket);
                }
                Some(read)
            });
            if let Some(read) = reread {
                self.send_pr_tab_read(id, read, cx);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
