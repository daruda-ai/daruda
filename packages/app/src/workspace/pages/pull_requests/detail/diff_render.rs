//! A file's diff, drawn unified or side by side: each row with its line
//! numbers, which select lines when pressed.

use daruda_content::diff::{VisualRow, VisualRowKind};
use gpui::{AnyElement, Context, Hsla, IntoElement, div, prelude::*, px};

use super::PrDetailId;
use super::view::ThreadsView;

mod inline;
use super::diff_layout::{Cell, LineRange, Side, SplitRow, line_on};
use super::files::{DIFF_ROWS_STEP, DiffMode, DiffRows, ShownView};
use crate::surface::strings::pull_requests as s;
use crate::ui::{button_compact, theme};
use crate::workspace::Workspace;
pub(super) use inline::unplaced_threads;

pub(super) fn diff_body(
    id: PrDetailId,
    shown: &ShownView,
    rows: &DiffRows,
    threads: &ThreadsView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let unified = rows.rows();
    // Lines are pressed to select them only once their commit is known.
    let selectable = shown.selectable;
    let selection = shown.selection.as_ref();
    let total = match shown.mode {
        DiffMode::Unified => unified.len(),
        DiffMode::Split => rows.split().len(),
    };
    let mut discussions = inline::positions(shown, rows, threads);
    let mut drawn = Vec::new();
    for ix in shown.start.min(total)..total.min(shown.limit) {
        let line = match shown.mode {
            DiffMode::Unified => {
                let row = &unified[ix];
                diff_row(
                    id,
                    ix,
                    row,
                    selection.is_some_and(|lines| lines.holds(row)),
                    selectable,
                    cx,
                )
            }
            DiffMode::Split => split_row(
                id,
                ix,
                &rows.split()[ix],
                unified,
                selection,
                selectable,
                cx,
            ),
        };
        drawn.push(line);
        if let Some(discussions) = discussions.remove(&ix) {
            drawn.push(inline::at_row(id, shown.mode, &discussions, cx));
        }
    }
    let rest = total.saturating_sub(shown.limit);
    let earlier = shown.start.min(total);
    div()
        .flex()
        .flex_col()
        .children((earlier > 0).then(|| {
            div()
                .flex()
                .justify_center()
                .py(px(theme::PAD_SM))
                .border_b_1()
                .border_color(t.border)
                .child(
                    button_compact(
                        "pr-file-diff-earlier",
                        s::diff_earlier_lines(earlier.min(DIFF_ROWS_STEP), earlier),
                    )
                    .debug_selector(|| "pr-file-diff-earlier".into())
                    .on_click(cx.listener(move |ws, _, _, cx| ws.show_earlier_pr_diff(id, cx))),
                )
        }))
        // Long lines scroll sideways rather than clip.
        .child(
            div()
                .id("pr-file-diff-scroll")
                .debug_selector(|| "pr-file-diff-scroll".into())
                .flex()
                .overflow_x_scroll()
                // A vertical wheel scrolls the detail, not this.
                .restrict_scroll_to_axis()
                // Unified rows take their longest line's width, and at least
                // the view's. Split halves share the view equally, each
                // clipping its own long lines, so old and new stay aligned.
                .child(
                    div()
                        .debug_selector(|| "pr-file-diff-rows".into())
                        .flex_none()
                        .flex()
                        .flex_col()
                        .min_w_full()
                        .when(shown.mode == DiffMode::Split, |rows| {
                            rows.w_full().min_w(px(2.0 * theme::PR_SPLIT_HALF_MIN_W))
                        })
                        .children(drawn),
                ),
        )
        .children((rest > 0).then(|| {
            div()
                .flex()
                .justify_center()
                .py(px(theme::PAD_SM))
                .border_t_1()
                .border_color(t.border)
                .child(
                    button_compact(
                        "pr-file-diff-more",
                        s::diff_more_lines(rest.min(DIFF_ROWS_STEP), rest),
                    )
                    .debug_selector(|| "pr-file-diff-more".into())
                    .on_click(cx.listener(move |ws, _, _, cx| ws.show_more_pr_diff(id, cx))),
                )
        }))
        .into_any_element()
}

/// What a row of `kind` is marked and coloured with.
fn row_style(kind: VisualRowKind, cx: &Context<Workspace>) -> (&'static str, Option<Hsla>, Hsla) {
    let t = theme::current(cx);
    match kind {
        VisualRowKind::Added => ("+", Some(t.file_diff_add_bg), t.file_diff_add_text),
        VisualRowKind::Removed => ("-", Some(t.file_diff_del_bg), t.file_diff_del_text),
        VisualRowKind::HunkHeader => ("", Some(t.overlay_hover), t.text_subtle),
        VisualRowKind::Context | VisualRowKind::Plain => (" ", None, t.text_body),
        VisualRowKind::NoNewline => ("", None, t.text_subtle),
    }
}

fn row_text(row: &VisualRow) -> String {
    match row.kind {
        VisualRowKind::HunkHeader if !row.header_context.is_empty() => {
            format!("{} {}", row.content, row.header_context)
        }
        _ => row.content.clone(),
    }
}

/// A line-number cell; pressing it selects the row's line, and with Shift
/// stretches the selection to it.
fn line_number(
    id: PrDetailId,
    key: (&'static str, usize),
    text: &str,
    line: Option<(Side, u32)>,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let cell = div()
        .id(key)
        .flex_none()
        .w(px(theme::PR_DIFF_LINE_NO_W))
        .pr(px(theme::PAD_SM))
        .text_right()
        .text_color(t.text_subtle)
        .child(text.to_owned());
    match line {
        Some((side, line)) => cell
            .cursor_pointer()
            .hover(|style| style.text_color(t.text_body))
            .on_click(cx.listener(move |ws, event: &gpui::ClickEvent, _, cx| {
                ws.select_pr_line(id, side, line, event.modifiers().shift, cx)
            }))
            .into_any_element(),
        None => cell.into_any_element(),
    }
}

fn diff_row(
    id: PrDetailId,
    ix: usize,
    row: &VisualRow,
    selected: bool,
    selectable: bool,
    cx: &Context<Workspace>,
) -> AnyElement {
    let (marker, bg, fg) = row_style(row.kind, cx);
    // Each number selects the line on its own side.
    let on = |side| {
        line_on(row, side)
            .filter(|_| selectable)
            .map(|line| (side, line))
    };
    let (left, right) = (on(Side::Old), on(Side::New));
    div()
        .font_family(theme::FONT_FAMILY_MONOSPACE)
        .text_size(px(theme::FONT_SIZE_SM))
        .debug_selector(move || format!("pr-diff-row-{ix}"))
        .flex()
        .w_full()
        .when_some(bg, |line, bg| line.bg(bg))
        .when(selected, |line| line.bg(theme::SELECTION_BG))
        .child(line_number(
            id,
            ("pr-diff-old", ix),
            &row.line_no_left,
            left,
            cx,
        ))
        .child(line_number(
            id,
            ("pr-diff-new", ix),
            &row.line_no_right,
            right,
            cx,
        ))
        .child(
            div()
                .flex_none()
                .w(px(theme::GAP_LG))
                .text_color(fg)
                .child(marker),
        )
        .child(
            div()
                .flex_none()
                .pr(px(theme::PAD_LG))
                .whitespace_nowrap()
                .text_color(fg)
                .child(row_text(row)),
        )
        .into_any_element()
}

/// A side-by-side row: the old line on the left, the new on the right.
fn split_row(
    id: PrDetailId,
    ix: usize,
    row: &SplitRow,
    unified: &[VisualRow],
    selection: Option<&LineRange>,
    selectable: bool,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let (old, new) = match row {
        SplitRow::Banner(at) => {
            let (_, bg, fg) = row_style(unified[*at].kind, cx);
            return div()
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .text_size(px(theme::FONT_SIZE_SM))
                .flex()
                .w_full()
                .pl(px(2.0 * theme::PR_DIFF_LINE_NO_W))
                .when_some(bg, |line, bg| line.bg(bg))
                .text_color(fg)
                .whitespace_nowrap()
                .child(row_text(&unified[*at]))
                .into_any_element();
        }
        SplitRow::Pair { old, new } => (old, new),
    };
    let half = |cell: &Option<Cell>, side: Side, key: &'static str| {
        let Some(cell) = cell else {
            return div()
                .flex_1()
                .flex_basis(px(0.0))
                .bg(t.overlay_hover)
                .into_any_element();
        };
        let row = &unified[cell.row];
        let (marker, bg, fg) = row_style(row.kind, cx);
        let number = match side {
            Side::Old => &row.line_no_left,
            Side::New => &row.line_no_right,
        };
        let line = line_on(row, side)
            .filter(|_| selectable)
            .map(|line| (side, line));
        let selected = selection.is_some_and(|lines| lines.side == side && lines.holds(row));
        div()
            .flex_1()
            .flex_basis(px(0.0))
            .min_w_0()
            .overflow_hidden()
            .flex()
            .when_some(bg, |half, bg| half.bg(bg))
            .when(selected, |half| half.bg(theme::SELECTION_BG))
            .child(line_number(id, (key, ix), number, line, cx))
            .child(
                div()
                    .flex_none()
                    .w(px(theme::GAP_LG))
                    .text_color(fg)
                    .child(marker),
            )
            .child(
                div()
                    .flex_none()
                    .pr(px(theme::PAD_LG))
                    .whitespace_nowrap()
                    .text_color(fg)
                    .child(row.content.clone()),
            )
            .into_any_element()
    };
    div()
        .font_family(theme::FONT_FAMILY_MONOSPACE)
        .text_size(px(theme::FONT_SIZE_SM))
        .debug_selector(move || format!("pr-split-row-{ix}"))
        .flex()
        .w_full()
        .child(half(old, Side::Old, "pr-split-old"))
        .child(
            div()
                .flex()
                .flex_1()
                .flex_basis(px(0.0))
                .min_w_0()
                .border_l_1()
                .border_color(t.border)
                .child(half(new, Side::New, "pr-split-new")),
        )
        .into_any_element()
}
