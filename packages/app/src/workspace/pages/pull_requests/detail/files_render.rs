//! The Files tab, drawn from its view: the changed files, each with how it
//! changed and by how many lines, and below them the chosen file's diff.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, UniformListScrollHandle, div,
    prelude::*, px,
};

use super::PrDetailId;
use super::diff_layout::LineRange;
use super::files::{
    ChangeKind, DiffMode, FileDiff, FileRowView, FilesMore, FilesView, RecoveryView, ShownView,
    Unchanged,
};
use super::progress::Viewed;
use super::recover::RecoverFailure;
use super::view::ReviewCommentsView;
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{
    Disableable as _, Selectable as _, button, button_compact, button_group, button_icon,
    button_with_icon, icons, theme, tooltip,
};
use crate::workspace::Workspace;

pub(super) fn files(
    id: PrDetailId,
    view: &FilesView,
    comments: Option<&ReviewCommentsView>,
    list_scroll: &UniformListScrollHandle,
    cx: &Context<Workspace>,
) -> AnyElement {
    let (files, total, viewed, more, shown) = match view {
        FilesView::Loading => return note(common::loading(), cx),
        FilesView::Failed(reason) => return note(s::detail_failed(reason), cx),
        FilesView::Ready {
            files,
            total,
            viewed,
            more,
            shown,
        } => (files, *total, *viewed, more, shown),
    };
    if files.is_empty() {
        return note(s::files_empty(), cx);
    }
    div()
        .flex()
        .flex_col()
        .gap(px(theme::GAP_LG))
        .child(progress(id, files.len(), total, viewed, more, cx))
        .child(file_list(id, files, list_scroll, cx))
        .children(
            shown
                .as_ref()
                .map(|shown| shown_diff(id, shown, comments, cx)),
        )
        .into_any_element()
}

/// How much of the listing is in, and what reads the rest.
fn progress(
    id: PrDetailId,
    read: usize,
    total: Option<u32>,
    viewed: usize,
    more: &FilesMore,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let count = match total {
        Some(total) => s::files_progress(read, total.max(read as u32)),
        None => s::files_progress_unknown(read),
    };
    let count = if viewed > 0 {
        s::files_viewed(count, viewed)
    } else {
        count
    };
    let (message, action) = match more {
        FilesMore::None => (count, None),
        FilesMore::Reading => (s::files_reading(count), None),
        FilesMore::Refreshing => (s::files_refreshing(count), None),
        FilesMore::RefreshFailed(reason) => {
            (s::files_refresh_failed(count, reason), Some(s::retry()))
        }
        FilesMore::Paused => (s::files_paused(count), Some(s::files_read_rest())),
        FilesMore::Failed(reason) => (s::files_failed(count, reason), Some(s::retry())),
        FilesMore::Capped => (s::files_truncated(count), None),
    };
    div()
        .debug_selector(|| "pr-files-progress".into())
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_LG))
        .text_size(px(theme::FONT_SIZE_SM))
        .text_color(
            if matches!(more, FilesMore::Failed(_) | FilesMore::RefreshFailed(_)) {
                t.banner_warning_text
            } else {
                t.text_muted
            },
        )
        .child(div().min_w_0().child(message))
        .children(action.map(|label| {
            button_compact("pr-files-resume", label)
                .debug_selector(|| "pr-files-resume".into())
                .on_click(cx.listener(move |ws, _, _, cx| ws.resume_pr_files(id, cx)))
        }))
        .into_any_element()
}

/// The listing, at most so tall, drawing only the rows in view: a pull
/// request can list thousands.
fn file_list(
    id: PrDetailId,
    files: &Rc<Vec<FileRowView>>,
    scroll: &UniformListScrollHandle,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let rows = files.clone();
    let count = rows.len();
    let height = (count as f32 * theme::PR_FILE_ROW_H).min(theme::PR_FILES_LIST_MAX_H);
    div()
        .debug_selector(|| "pr-files".into())
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        .child(
            gpui::uniform_list(
                "pr-files-list",
                count,
                cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| Some(file_row(id, index, rows.get(index)?, cx)))
                        .collect()
                }),
            )
            .track_scroll(scroll)
            .w_full()
            .h(px(height)),
        )
        .into_any_element()
}

pub(super) fn note(message: String, cx: &Context<Workspace>) -> AnyElement {
    div()
        .text_size(px(theme::FONT_SIZE_MD))
        .text_color(theme::current(cx).text_muted)
        .child(message)
        .into_any_element()
}

fn file_row(
    id: PrDetailId,
    index: usize,
    file: &FileRowView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let (glyph, color) = match file.kind {
        ChangeKind::Added => ("A", t.file_diff_stat_add),
        ChangeKind::Removed => ("D", t.file_diff_stat_del),
        ChangeKind::Modified => ("M", t.text_muted),
        ChangeKind::Renamed => ("R", t.text_muted),
        ChangeKind::Other => ("·", t.text_subtle),
    };
    let path = file.path.clone();
    // The mark sits beside the row's pressable part, so marking a file
    // does not also show it.
    let row = div()
        .id(("pr-file", index))
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .items_center()
        .gap(px(theme::GAP_LG))
        .pr(px(theme::PAD_LG))
        .cursor_pointer()
        .when(!file.shown, |row| {
            row.hover(|style| style.bg(t.overlay_hover))
        })
        .when(file.viewed == Viewed::Yes, |row| {
            row.opacity(theme::PR_FILE_VIEWED_OPACITY)
        })
        .child(
            div()
                .flex_none()
                .w(px(theme::CONTROL_ICON_SIZE))
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(glyph),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(theme::GAP_LG))
                .child(
                    div()
                        .id(("pr-file-path", index))
                        .min_w_0()
                        .truncate()
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_color(t.text_body)
                        .tooltip(tooltip::text(file.path.clone()))
                        .child(file.path.clone()),
                )
                .children(
                    file.note
                        .clone()
                        // The path keeps the room; the note takes what is left.
                        .map(|note| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text_subtle)
                                .child(note)
                        }),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .gap(px(theme::GAP_SM))
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .child(
                    div()
                        .text_color(t.file_diff_stat_add)
                        .child(format!("+{}", file.additions)),
                )
                .child(
                    div()
                        .text_color(t.file_diff_stat_del)
                        .child(format!("-{}", file.deletions)),
                ),
        )
        .on_click(cx.listener(move |ws, _, _, cx| ws.show_pr_file(id, &path, cx)));
    div()
        .debug_selector(move || format!("pr-file-{index}"))
        .w_full()
        .flex()
        .items_center()
        .h(px(theme::PR_FILE_ROW_H))
        .gap(px(theme::GAP_SM))
        .pl(px(theme::PAD_SM))
        .text_size(px(theme::FONT_SIZE_SM))
        .when(file.shown, |row| row.bg(t.lane_card_active_bg))
        .child(viewed_mark(id, ("pr-file-viewed", index), file, cx))
        .child(row)
        .into_any_element()
}

/// The box that marks a file viewed, or says its content changed since.
fn viewed_mark(
    id: PrDetailId,
    key: (&'static str, usize),
    file: &FileRowView,
    cx: &Context<Workspace>,
) -> AnyElement {
    let (icon, tip) = match file.viewed {
        Viewed::No => (icons::CHECKBOX_OFF, s::file_mark_viewed()),
        Viewed::Yes => (icons::CHECKBOX_ON, s::file_unmark_viewed()),
        Viewed::Changed => (icons::CHECKBOX_MIXED, s::file_changed_since_viewed()),
    };
    let path = file.path.clone();
    let (name, index) = key;
    button_icon(key, icon, cx)
        .debug_selector(move || format!("{name}-{index}"))
        .tooltip(tip)
        .disabled(!file.markable)
        .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_pr_file_viewed(id, &path, cx)))
        .into_any_element()
}

fn shown_diff(
    id: PrDetailId,
    shown: &ShownView,
    comments: Option<&ReviewCommentsView>,
    cx: &Context<Workspace>,
) -> AnyElement {
    let composer = comments.and_then(|comments| comments.composer.as_ref());
    let t = theme::current(cx);
    let body = match &shown.diff {
        FileDiff::TooLarge => too_large(id, shown.recovery.as_ref(), cx),
        FileDiff::NoLines => note(s::file_no_lines(), cx),
        FileDiff::MovedOnly => note(s::file_moved_only(), cx),
        FileDiff::Rows(rows) => super::diff_render::diff_body(id, shown, rows, cx),
    };
    div()
        .debug_selector(|| "pr-file-diff".into())
        .flex()
        .flex_col()
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        .child(diff_header(id, shown, cx))
        .children(
            shown
                .selection
                .as_ref()
                .map(|lines| selection_bar(id, lines, composer.is_none(), cx)),
        )
        .child(body)
        .into_any_element()
}

/// GitHub sent no patch: say so, and offer to read the file whole.
fn too_large(
    id: PrDetailId,
    recovery: Option<&RecoveryView>,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let (message, action) = match recovery {
        None => (s::file_too_large(), None),
        Some(RecoveryView::Offered) => (s::file_too_large(), Some(s::file_recover())),
        Some(RecoveryView::Busy) => (s::file_too_large(), None),
        Some(RecoveryView::Reading) => (s::file_recovering(), None),
        Some(RecoveryView::Failed(failure)) => (
            match failure {
                RecoverFailure::Binary => s::file_recover_binary(),
                RecoverFailure::TooLarge => s::file_recover_too_large(),
                RecoverFailure::Failed(reason) => s::file_recover_failed(reason),
            },
            Some(s::retry()),
        ),
    };
    div()
        .debug_selector(|| "pr-file-too-large".into())
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_LG))
        .p(px(theme::PAD_LG))
        .text_size(px(theme::FONT_SIZE_MD))
        .text_color(t.text_muted)
        .child(div().min_w_0().child(message))
        .children(action.map(|label| {
            button_compact("pr-file-recover", label)
                .debug_selector(|| "pr-file-recover".into())
                .on_click(cx.listener(move |ws, _, _, cx| ws.recover_pr_file(id, cx)))
        }))
        .into_any_element()
}

/// The shown file's path and place, the previous and next file, and how
/// its diff is laid out.
fn diff_header(id: PrDetailId, shown: &ShownView, cx: &Context<Workspace>) -> AnyElement {
    let t = theme::current(cx);
    // Unplaced while a refresh has not listed it again: nothing to step from.
    let (at, of) = shown.position.unwrap_or((0, 0));
    let modes = [DiffMode::Unified, DiffMode::Split];
    let folded = shown.unchanged == Unchanged::Folded;
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_SM))
        .px(px(theme::PAD_LG))
        .py(px(theme::PAD_XS))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_muted)
                .child(SharedString::from(shown.path.clone())),
        )
        .children(shown.position.map(|(at, of)| {
            div()
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_subtle)
                .child(s::file_position(at + 1, of))
        }))
        .children(shown.row.as_ref().map(|row| {
            let path = row.path.clone();
            button_with_icon(
                "pr-file-viewed-shown",
                s::file_viewed(),
                match row.viewed {
                    Viewed::No => icons::CHECKBOX_OFF,
                    Viewed::Yes => icons::CHECKBOX_ON,
                    Viewed::Changed => icons::CHECKBOX_MIXED,
                },
            )
            .debug_selector(|| "pr-file-viewed-shown".into())
            .tooltip(match row.viewed {
                Viewed::No => s::file_mark_viewed(),
                Viewed::Yes => s::file_unmark_viewed(),
                Viewed::Changed => s::file_changed_since_viewed(),
            })
            .disabled(!row.markable)
            .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_pr_file_viewed(id, &path, cx)))
        }))
        .child(
            button_icon("pr-file-previous", icons::PREVIOUS, cx)
                .debug_selector(|| "pr-file-previous".into())
                .tooltip(s::file_previous())
                .disabled(shown.position.is_none() || at == 0)
                .on_click(cx.listener(move |ws, _, _, cx| ws.show_adjacent_pr_file(id, -1, cx))),
        )
        .child(
            button_icon("pr-file-next", icons::NEXT, cx)
                .debug_selector(|| "pr-file-next".into())
                .tooltip(s::file_next())
                .disabled(at + 1 >= of)
                .on_click(cx.listener(move |ws, _, _, cx| ws.show_adjacent_pr_file(id, 1, cx))),
        )
        .child(
            button_icon(
                "pr-file-unchanged",
                if folded {
                    icons::UNFOLD_MORE
                } else {
                    icons::UNFOLD_LESS
                },
                cx,
            )
            .debug_selector(|| "pr-file-unchanged".into())
            .tooltip(if folded {
                s::diff_show_unchanged()
            } else {
                s::diff_fold_unchanged()
            })
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.set_pr_diff_unchanged(
                    id,
                    if folded {
                        Unchanged::Shown
                    } else {
                        Unchanged::Folded
                    },
                    cx,
                )
            })),
        )
        .child(
            button_group("pr-diff-mode", cx)
                .children(modes.into_iter().enumerate().map(|(index, mode)| {
                    button(
                        ("pr-diff-mode", index),
                        match mode {
                            DiffMode::Unified => s::diff_unified(),
                            DiffMode::Split => s::diff_split(),
                        },
                    )
                    .selected(shown.mode == mode)
                    .debug_selector(move || format!("pr-diff-mode-{index}"))
                }))
                .on_click(cx.listener(move |ws, indices: &Vec<usize>, _, cx| {
                    if let Some(mode) = indices.first().and_then(|ix| modes.get(*ix)) {
                        ws.set_pr_diff_mode(id, *mode, cx);
                    }
                })),
        )
        .into_any_element()
}

/// The selected lines' reference, to copy or clear.
fn selection_bar(
    id: PrDetailId,
    lines: &LineRange,
    can_comment: bool,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    div()
        .debug_selector(|| "pr-lines-selected".into())
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(theme::GAP_LG))
        .px(px(theme::PAD_LG))
        .py(px(theme::PAD_XS))
        .border_b_1()
        .border_color(t.border)
        .bg(theme::SELECTION_BG)
        .text_size(px(theme::FONT_SIZE_SM))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(theme::FONT_FAMILY_MONOSPACE)
                .text_color(t.text_body)
                .child(lines.reference()),
        )
        .when(can_comment, |bar| {
            bar.child(
                button_compact("pr-lines-comment", s::lines_comment())
                    .debug_selector(|| "pr-lines-comment".into())
                    .on_click(
                        cx.listener(move |ws, _, window, cx| ws.comment_pr_lines(id, window, cx)),
                    ),
            )
        })
        .child(
            button_compact("pr-lines-copy", s::lines_copy())
                .debug_selector(|| "pr-lines-copy".into())
                .on_click(cx.listener(move |ws, _, _, cx| ws.copy_pr_lines(id, cx))),
        )
        .child(
            button_compact("pr-lines-clear", s::lines_clear())
                .debug_selector(|| "pr-lines-clear".into())
                .on_click(cx.listener(move |ws, _, _, cx| ws.clear_pr_lines(id, cx))),
        )
        .into_any_element()
}
