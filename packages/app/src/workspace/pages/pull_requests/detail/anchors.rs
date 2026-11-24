//! Landing a GitHub comment link after its tab has read the conversation,
//! then revealing the exact rendered comment once its layout is known.

use daruda_github::ReviewComment;
use gpui::{Bounds, Context, Div, Pixels, canvas, point, prelude::*, px};

use super::view::{CommentView, ThreadTarget};
use super::{Load, OpeningTarget, PrDetailId, PrDetailState, PrTab};
use crate::ui::theme;
use crate::workspace::Workspace;

#[derive(Debug, Clone, Copy)]
pub(in crate::workspace) enum CommentKind {
    Review(u64),
    Conversation(u64),
}

impl CommentKind {
    pub(super) fn tab(self) -> PrTab {
        match self {
            Self::Review(_) => PrTab::Threads,
            Self::Conversation(_) => PrTab::Description,
        }
    }

    fn matches(self, comment: &ReviewComment) -> bool {
        let Some((_, fragment)) = comment.web_url().and_then(|url| url.rsplit_once('#')) else {
            return false;
        };
        let (prefix, number) = match self {
            Self::Review(id) => ("discussion_r", id),
            Self::Conversation(id) => ("issuecomment-", id),
        };
        fragment
            .strip_prefix(prefix)
            .and_then(|id| id.parse::<u64>().ok())
            == Some(number)
    }
}

#[derive(Debug, Clone)]
pub(in crate::workspace) struct CommentLink {
    pub(in crate::workspace) kind: CommentKind,
    pub(in crate::workspace) url: String,
}

enum Located {
    Waiting,
    Missing,
    Conversation(String),
    Thread {
        thread: String,
        target: Option<ThreadTarget>,
        comment: String,
    },
}

impl CommentLink {
    fn locate(&self, detail: &PrDetailState) -> Located {
        match self.kind {
            CommentKind::Conversation(_) => {
                let Some(slot) = &detail.conversation else {
                    return Located::Waiting;
                };
                if let Some(read) = slot.confirmed() {
                    return read
                        .comments
                        .iter()
                        .find(|c| self.kind.matches(c))
                        .map_or(Located::Missing, |c| Located::Conversation(c.id.clone()));
                }
                missing_unless_waiting(slot)
            }
            CommentKind::Review(_) => {
                let Some(slot) = &detail.threads else {
                    return Located::Waiting;
                };
                if let Some(read) = slot.confirmed() {
                    return read
                        .threads
                        .iter()
                        .find_map(|thread| {
                            let comment = thread.comments.iter().find(|c| self.kind.matches(c))?;
                            Some(Located::Thread {
                                thread: thread.id.clone(),
                                target: ThreadTarget::of(thread, detail.thread_head.as_deref()),
                                comment: comment.id.clone(),
                            })
                        })
                        .unwrap_or(Located::Missing);
                }
                missing_unless_waiting(slot)
            }
        }
    }
}

/// Not found in a read that failed, unless a newer read is still out: a
/// retry started for the link answers it later.
fn missing_unless_waiting<T>(slot: &super::read_slot::ReadSlot<T>) -> Located {
    if slot.waiting().is_none() && matches!(slot.load(), Load::Failed(_)) {
        Located::Missing
    } else {
        Located::Waiting
    }
}

pub(super) enum CommentLocation {
    Looking(CommentLink),
    /// Its thread was found. Wait for the diff to say whether it has the lines.
    Lines {
        target: ThreadTarget,
        comment: String,
    },
    Reveal(String),
    Highlight(String),
    Missing(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::workspace) enum CommentMark {
    Plain,
    Reveal,
    Highlight,
}

impl PrDetailState {
    pub(super) fn comment_mark(&self, id: &str) -> CommentMark {
        match &self.comment_location {
            Some(CommentLocation::Reveal(found)) if found == id => CommentMark::Reveal,
            Some(CommentLocation::Highlight(found)) if found == id => CommentMark::Highlight,
            _ => CommentMark::Plain,
        }
    }
}

impl Workspace {
    pub(super) fn apply_pr_opening(
        &mut self,
        id: PrDetailId,
        at: Option<OpeningTarget>,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = at else { return };
        self.select_pr_detail_tab(id, at.tab(), cx);
        if let OpeningTarget::Comment(link) = at
            && let Some(detail) = self.pr_detail_mut(id)
        {
            // A link may name a reply posted since this open detail's cached read.
            let read = if matches!(link.locate(detail), Located::Missing) {
                detail.reread_tab(link.kind.tab())
            } else {
                None
            };
            detail.comment_location = Some(CommentLocation::Looking(link));
            if let Some(read) = read {
                self.send_pr_tab_read(id, read, cx);
            } else {
                self.follow_pr_comment(id, cx);
            }
        }
    }

    pub(in crate::workspace) fn reveal_pr_comment(
        &mut self,
        id: PrDetailId,
        comment: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.comment_location = Some(CommentLocation::Reveal(comment));
            cx.notify();
        }
    }

    pub(super) fn reveal_pr_thread_in_diff(
        &mut self,
        id: PrDetailId,
        target: ThreadTarget,
        comment: String,
        cx: &mut Context<Self>,
    ) {
        self.show_pr_thread_in_diff(id, target.clone(), cx);
        if let Some(detail) = self.pr_detail_mut(id) {
            detail.comment_location = Some(CommentLocation::Lines { target, comment });
        }
        self.follow_pr_comment(id, cx);
    }

    pub(super) fn follow_pr_comment(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        match detail.comment_location.as_ref() {
            Some(CommentLocation::Looking(link)) => match link.locate(detail) {
                Located::Waiting => {}
                Located::Missing => {
                    detail.comment_location = Some(CommentLocation::Missing(link.url.clone()));
                    // A reused detail could be scrolled deep into another tab.
                    detail.scroll.set_offset(Default::default());
                }
                Located::Conversation(comment) => self.reveal_pr_comment(id, comment, cx),
                Located::Thread {
                    thread,
                    target,
                    comment,
                } => {
                    detail.expanded_threads.insert(thread);
                    if let Some(target) = target {
                        self.reveal_pr_thread_in_diff(id, target, comment, cx);
                    } else {
                        self.reveal_pr_comment(id, comment, cx);
                    }
                }
            },
            Some(CommentLocation::Lines { target, comment }) => {
                let Some(placed) = detail
                    .files()
                    .and_then(|files| files.thread_placement(target))
                else {
                    return;
                };
                let comment = comment.clone();
                if !placed {
                    self.select_pr_detail_tab(id, PrTab::Threads, cx);
                }
                self.reveal_pr_comment(id, comment, cx);
            }
            _ => {}
        }
    }

    fn reveal_laid_out_comment(
        &mut self,
        id: PrDetailId,
        comment: String,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        if detail.comment_mark(&comment) != CommentMark::Reveal {
            return;
        }
        let scroll = detail.scroll();
        let offset = scroll.offset();
        scroll.set_offset(point(
            offset.x,
            offset.y + scroll.bounds().top() - bounds.top(),
        ));
        detail.comment_location = Some(CommentLocation::Highlight(comment));
        cx.notify();
    }
}

/// The marker measures the rendered comment once. The detail applies the
/// scroll after layout and ignores it if another link replaced this target.
pub(super) fn mark_comment(
    block: Div,
    id: PrDetailId,
    comment: &CommentView,
    cx: &Context<Workspace>,
) -> Div {
    if comment.mark == CommentMark::Plain {
        return block;
    }
    let key = comment.id.clone();
    let ws = cx.weak_entity();
    block
        .relative()
        .border_l_2()
        .border_color(theme::PRIMARY)
        .pl(px(theme::PAD_SM))
        .debug_selector(|| "pr-linked-comment".into())
        .when(comment.mark == CommentMark::Reveal, |block| {
            block.child(
                canvas(
                    move |bounds, _, cx| {
                        cx.defer(move |cx| {
                            // SILENT-OK: the workspace closed after laying out the comment.
                            let _ = ws.update(cx, |ws, cx| {
                                ws.reveal_laid_out_comment(id, key, bounds, cx)
                            });
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
        })
}

#[cfg(test)]
mod tests;
