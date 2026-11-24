//! Reading the review threads, and moving between a thread's place in the
//! Threads tab and its lines in the diff.

use daruda_github::{GhError, ReviewThreads};
use gpui::{Context, Window};

use super::{
    Load, PrDetailId, PrDetailState, PrTab, ReadSlot, ReadTicket, split, thread_actions, view,
};
use crate::gh_host::GhHost;
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

impl PrDetailState {
    /// The read the threads' slot waits on, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn threads_ticket(&self) -> Option<ReadTicket> {
        self.threads.as_ref()?.waiting()
    }

    pub(in crate::workspace) fn threads(&self) -> Option<&Load<ReviewThreads>> {
        self.threads.as_ref().map(ReadSlot::load)
    }
}

impl Workspace {
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

    /// Keep the same expanded discussion in view when moving between tabs.
    pub(in crate::workspace) fn jump_pr_thread(
        &mut self,
        id: PrDetailId,
        thread: &str,
        tab: PrTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(item) = detail
            .threads
            .as_ref()
            .and_then(ReadSlot::last_known)
            .and_then(|threads| threads.threads.iter().find(|item| item.id == thread))
        else {
            return;
        };
        let comment = item.comments.first().map(|comment| comment.id.clone());
        let target = view::ThreadTarget::of(item, detail.thread_head.as_deref());
        detail.expanded_threads.insert(thread.to_owned());
        detail.thread_work.focus_after_jump(thread, window, cx);
        if let (PrTab::Files, Some(target), Some(comment)) = (tab, target, comment.as_ref()) {
            self.reveal_pr_thread_in_diff(id, target, comment.clone(), cx);
        } else {
            self.select_pr_detail_tab(id, PrTab::Threads, cx);
            if let Some(comment) = comment {
                self.reveal_pr_comment(id, comment, cx);
            }
        }
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
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(files) = detail.files.as_mut() else {
            return;
        };
        files.go_to_lines(target);
        detail.reveal_file_row();
        cx.notify();
    }

    pub(super) fn send_pr_threads_read(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        cx: &mut Context<Self>,
    ) {
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
            Load::Ready(threads) if error.is_none() => Some(threads),
            Load::Ready(_) | Load::Loading | Load::Failed(_) => None,
        };
        if let Some(threads) = read {
            detail.thread_head = detail.pr.confirmed().map(|pr| pr.head_sha.clone());
            let open: std::collections::HashSet<&str> = threads
                .threads
                .iter()
                .map(|thread| thread.id.as_str())
                .collect();
            detail
                .expanded_threads
                .retain(|thread| open.contains(thread.as_str()));
        }
        let settled = detail.thread_work.threads_read(ticket, read);
        for (thread, how) in settled {
            match how {
                thread_actions::Settled::Posted => {
                    // What the box still holds is new text, not the reply
                    // that went out: it must not stay marked as sent.
                    self.let_go_pr_reply_sent(id, &thread, cx);
                    let left = self.pr_reply_left(id, &thread, cx);
                    self.set_pr_reply_draft(id, &thread, left, cx);
                }
                thread_actions::Settled::NotPosted => self.let_go_pr_reply_sent(id, &thread, cx),
            }
        }
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_threads.read", cx);
        }
        self.follow_pr_comment(id, cx);
        cx.notify();
    }
}
