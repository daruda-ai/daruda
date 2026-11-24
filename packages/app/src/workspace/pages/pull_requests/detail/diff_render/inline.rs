//! Review discussions anchored to source lines and conversations without a visible line.

use gpui::{AnyElement, Context, IntoElement, div, prelude::*, px};
use std::collections::HashMap;

use super::super::diff_layout::Side;
use super::super::files::{DiffMode, DiffRows, ShownView};
use super::super::threads_render::{self, ThreadPlace};
use super::super::view::{ThreadRowView, ThreadsView};
use super::super::{PrDetailId, files_render};
use crate::github::DraftKeeping;
use crate::surface::strings::pull_requests as s;
use crate::ui::theme;
use crate::workspace::Workspace;

pub(super) struct InlineThread<'a> {
    side: Side,
    index: usize,
    thread: &'a ThreadRowView,
    keeping: DraftKeeping,
}

/// Each thread belongs immediately after the last selected line on its own side.
pub(super) fn positions<'a>(
    shown: &ShownView,
    rows: &DiffRows,
    threads: &'a ThreadsView,
) -> HashMap<usize, Vec<InlineThread<'a>>> {
    let mut discussions = HashMap::<usize, Vec<_>>::new();
    if let ThreadsView::Listed {
        rows: threads,
        keeping,
        ..
    } = threads
    {
        for (index, thread) in threads.iter().enumerate() {
            if let Some(target) = thread
                .target
                .as_ref()
                .filter(|target| target.path == shown.path)
                && let Some(line) = rows.row_index(target.side, target.end, shown.mode)
                && shown.draws(line)
            {
                discussions.entry(line).or_default().push(InlineThread {
                    side: target.side,
                    index,
                    thread,
                    keeping: *keeping,
                });
            }
        }
    }
    discussions
}

pub(super) fn at_row(
    id: PrDetailId,
    mode: DiffMode,
    discussions: &[InlineThread],
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let render = |side: Option<Side>| {
        discussions
            .iter()
            .filter(|entry| side.is_none_or(|side| side == entry.side))
            .map(|entry| inline_thread(id, entry.index, entry.thread, entry.keeping, cx))
            .collect::<Vec<_>>()
    };
    let block = match mode {
        DiffMode::Unified => div().flex().flex_col().children(render(None)),
        DiffMode::Split => div().flex().children([Side::Old, Side::New].map(|side| {
            div()
                .flex_1()
                .flex_basis(px(0.))
                .min_w_0()
                .when(side == Side::New, |half| {
                    half.border_l_1().border_color(t.border)
                })
                .flex()
                .flex_col()
                .children(render(Some(side)))
        })),
    };
    block.into_any_element()
}

fn inline_thread(
    id: PrDetailId,
    index: usize,
    thread: &ThreadRowView,
    keeping: DraftKeeping,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    div()
        .debug_selector(move || format!("pr-inline-thread-{index}"))
        .w_full()
        .min_w_0()
        .max_w(px(theme::PR_INLINE_THREAD_MAX_W))
        .border_l_1()
        .border_color(t.border)
        .bg(t.dock_bg)
        .child(threads_render::thread_row(
            id,
            index,
            thread,
            keeping,
            ThreadPlace::Inline,
            cx,
        ))
        .into_any_element()
}

/// Keep outdated, file-level, and unavailable lines alongside the file
/// without attributing their comments to a current line.
pub(in super::super) fn unplaced_threads(
    id: PrDetailId,
    shown: &ShownView,
    threads: &ThreadsView,
    diff: Option<&DiffRows>,
    cx: &Context<Workspace>,
) -> Option<AnyElement> {
    let (rows, keeping) = match threads {
        ThreadsView::Listed { rows, keeping, .. } => (rows, *keeping),
        ThreadsView::Failed(reason) => {
            return Some(files_render::note(s::threads_failed(reason), cx));
        }
        ThreadsView::Loading => return None,
    };
    let mut unplaced = rows
        .iter()
        .enumerate()
        .filter_map(|(index, thread)| {
            if thread.path != shown.path {
                return None;
            }
            let anchor = thread
                .target
                .as_ref()
                .and_then(|target| diff?.row_index(target.side, target.end, shown.mode));
            match anchor {
                Some(ix) if shown.draws(ix) => None,
                Some(_) => Some((index, thread, ThreadPlace::BeyondLimit)),
                None => Some((index, thread, ThreadPlace::FileConversations)),
            }
        })
        .peekable();
    unplaced.peek()?;
    let t = theme::current(cx);
    Some(
        div()
            .debug_selector(|| "pr-file-unplaced-threads".into())
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(t.border)
            .p(px(theme::PAD_STANDARD))
            .gap(px(theme::GAP_SM))
            .text_size(px(theme::FONT_SIZE_SM))
            .text_color(t.text_muted)
            .child(s::thread_unplaced())
            .children(unplaced.map(|(index, row, place)| {
                threads_render::thread_row(id, index, row, keeping, place, cx)
            }))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    // Rendered row positions, sides, stale lines, and the shared reply editor
    // are exercised together in workspace::tests::pull_request_threads.
}
