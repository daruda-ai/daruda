//! The Threads tab: open review threads, each as an excerpt or in full.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ThreadRowView {
    /// GitHub's id for the thread, which expanding it names.
    pub id: String,
    pub path: String,
    /// `path:L12–L20`.
    pub location: String,
    pub author: String,
    /// The first line of the first comment.
    pub excerpt: String,
    pub replies: Option<String>,
    pub outdated: bool,
    /// The thread's first comment on GitHub.
    pub url: Option<String>,
    /// Where it sits in the current diff; `None` for an outdated thread,
    /// whose lines are where the code was, or a file-level one.
    pub target: Option<ThreadTarget>,
    /// Every comment in full, once the thread is expanded.
    pub comments: Option<Vec<CommentView>>,
    /// Resolved from this detail since the threads were read; it can be
    /// unresolved again until they are read anew.
    pub resolved_here: bool,
    /// The reply being written, if a box is open.
    pub reply: Option<gpui::Entity<crate::ui::TextareaState>>,
    pub navigation_focus: Option<gpui::FocusHandle>,
    /// The thread's last action, running or failed.
    pub op: Option<super::super::thread_actions::ThreadOp>,
    /// What the viewer may do to it.
    pub may: daruda_github::ThreadPermissions,
    /// A reply to it is kept in draft.
    pub draft: bool,
}

/// Lines of the diff at `revision` a thread is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ThreadTarget {
    pub path: String,
    pub side: super::super::diff_layout::Side,
    pub start: u32,
    pub end: u32,
    /// The head the lines were read at: a diff at another names other code.
    pub revision: String,
}

impl ThreadTarget {
    /// Where `thread`, read with the pull request at `head`, sits; `None`
    /// when outdated, on no line, or read at no known head.
    pub(in crate::workspace) fn of(thread: &ReviewThread, head: Option<&str>) -> Option<Self> {
        let head = head?;
        thread
            .lines
            .filter(|_| !thread.outdated)
            .map(|(start, end)| ThreadTarget {
                path: thread.path.clone(),
                side: match thread.side {
                    daruda_github::DiffSide::Left => super::super::diff_layout::Side::Old,
                    daruda_github::DiffSide::Right => super::super::diff_layout::Side::New,
                },
                start,
                end,
                revision: head.to_owned(),
            })
    }
}

/// One comment of an expanded thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct CommentView {
    pub mark: super::super::anchors::CommentMark,
    /// GitHub's id, which keys its drawn Markdown.
    pub id: String,
    pub author: String,
    /// Markdown, drawn as such.
    pub body: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum ThreadsView {
    Loading,
    Failed(String),
    Listed {
        rows: Vec<ThreadRowView>,
        truncated: bool,
        /// Whether replies typed now are kept across restarts.
        keeping: crate::github::DraftKeeping,
        /// Reply drafts kept for threads no longer listed: thread id and
        /// text, shown so they can be copied before being discarded.
        orphan_drafts: Vec<(String, String)>,
    },
}

/// Unresolved threads, and whether there are more than that: the threads
/// once read in full, else the detail's count of the first hundred.
pub(super) fn open_threads(state: &PrDetailState, loaded: &PrDetail) -> (usize, bool) {
    match state.threads() {
        Some(Load::Ready(threads)) => (
            threads
                .threads
                .iter()
                .filter(|thread| !state.thread_work().resolved_here(&thread.id))
                .count(),
            threads.truncated,
        ),
        _ => (loaded.open_threads as usize, loaded.open_threads_more),
    }
}

pub(super) fn threads_view(
    threads: &ReviewThreads,
    state: &PrDetailState,
    drafts: Option<&daruda_store::review_drafts::PrDrafts>,
    keeping: crate::github::DraftKeeping,
) -> ThreadsView {
    let in_list = state.tab() == PrTab::Threads;
    let files = state.files().filter(|_| state.tab() == PrTab::Files);
    let path = files.and_then(|files| files.shown_path());
    let same_revision = state.thread_head.as_deref().is_some_and(|head| {
        state.pr.last_known().is_some_and(|pr| pr.head_sha == head)
            && files.is_none_or(|files| files.shown_at(head))
    });
    let replies = drafts
        .map(|drafts| drafts.replies.as_slice())
        .unwrap_or_default();
    let drafted = |thread: &str| replies.iter().any(|held| held.thread == thread);
    ThreadsView::Listed {
        keeping,
        // Kept for threads no longer listed: resolved or gone since. With
        // the list cut short, an unlisted thread may still be open.
        orphan_drafts: if !in_list || threads.truncated {
            Vec::new()
        } else {
            replies
                .iter()
                .filter(|held| {
                    !threads
                        .threads
                        .iter()
                        .any(|thread| thread.id == held.thread)
                })
                .map(|held| (held.thread.clone(), held.text.clone()))
                .collect()
        },
        rows: threads
            .threads
            .iter()
            .filter(|thread| in_list || path == Some(thread.path.as_str()))
            .map(|thread| {
                let mut row = thread_row(thread, state.thread_head.as_deref());
                if !same_revision {
                    row.target = None;
                }
                if state.thread_expanded(&thread.id) {
                    row.comments = Some(
                        thread
                            .comments
                            .iter()
                            .map(|c| comment_view(c, state))
                            .collect(),
                    );
                }
                let work = state.thread_work();
                row.resolved_here = work.resolved_here(&thread.id);
                row.draft = drafted(&thread.id);
                if let Some(work) = work.get(&thread.id) {
                    row.reply = work.reply.clone();
                    row.navigation_focus = work.navigation_focus.clone();
                    row.op = work.op.clone();
                }
                row
            })
            .collect(),
        truncated: threads.truncated,
    }
}

pub(super) fn comment_view(
    comment: &daruda_github::ReviewComment,
    state: &PrDetailState,
) -> CommentView {
    CommentView {
        mark: state.comment_mark(&comment.id),
        id: comment.id.clone(),
        author: comment.author.clone().unwrap_or_else(s::author_deleted),
        body: comment.body.clone(),
        url: comment.web_url().map(str::to_owned),
    }
}

pub(super) fn thread_row(thread: &ReviewThread, head: Option<&str>) -> ThreadRowView {
    let location = match thread.lines {
        Some((first, last)) if first == last => s::thread_line(&thread.path, first),
        Some((first, last)) => s::thread_lines(&thread.path, first, last),
        None => thread.path.clone(),
    };
    let first = thread.comments.first();
    let replies = thread.comments.len().saturating_sub(1);
    ThreadRowView {
        path: thread.path.clone(),
        location,
        author: first
            .and_then(|comment| comment.author.clone())
            .unwrap_or_else(s::author_deleted),
        excerpt: first
            .map(|comment| excerpt(&comment.body))
            .unwrap_or_default(),
        replies: match (replies, thread.more_comments) {
            (_, true) => Some(s::thread_replies_more(replies)),
            (0, false) => None,
            (1, false) => Some(s::thread_one_reply()),
            (n, false) => Some(s::thread_replies(n)),
        },
        outdated: thread.outdated,
        id: thread.id.clone(),
        url: first.and_then(|comment| comment.web_url().map(str::to_owned)),
        target: ThreadTarget::of(thread, head),
        comments: None,
        resolved_here: false,
        reply: None,
        navigation_focus: None,
        op: None,
        may: thread.may,
        draft: false,
    }
}

/// The first line with text in it, without control characters: a reviewer's
/// text is drawn as it came.
pub(super) fn excerpt(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}
