//! The Files review surface, with a changed-file navigator or a compact
//! picker sharing the same selected diff and review progress.

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
use super::view::{ReadyView, SendSlot};
use crate::surface::strings::{common, pull_requests as s};
use crate::ui::{
    Disableable as _, Selectable as _, button, button_compact, button_group, button_icon,
    button_with_icon, icons, theme, tooltip,
};
use crate::workspace::Workspace;

pub(super) fn files(
    id: PrDetailId,
    ready: &ReadyView,
    pr_url: Option<&str>,
    list_scroll: &UniformListScrollHandle,
    wide: bool,
    cx: &Context<Workspace>,
) -> AnyElement {
    let review = ready
        .review_comments
        .as_ref()
        .and_then(|comments| super::review_comments_render::pending(id, comments, cx));
    let panel = div().flex().flex_col().min_w_0().gap(px(theme::GAP_LG));
    let (files, total, viewed, more, shown, picker_open) = match ready.files.as_ref() {
        None | Some(FilesView::Loading) => {
            return panel
                .children(review)
                .child(note(common::loading(), cx))
                .into_any_element();
        }
        Some(FilesView::Failed(reason)) => {
            return panel
                .children(review)
                .child(note(s::detail_failed(reason), cx))
                .into_any_element();
        }
        Some(FilesView::Ready {
            files,
            total,
            viewed,
            more,
            shown,
            picker_open,
        }) => (files, *total, *viewed, more, shown.as_deref(), *picker_open),
    };
    if files.is_empty() {
        return panel
            .children(review)
            .child(note(s::files_empty(), cx))
            .into_any_element();
    }
    let picker = (!wide).then(|| {
        let label = shown.map_or_else(s::file_choose, |file| file.path.clone());
        button_with_icon("pr-files-picker", label, icons::EXPAND_MORE)
            .max_w_full()
            .tooltip(s::file_choose())
            .debug_selector(|| "pr-files-picker".into())
            .on_click(cx.listener(move |ws, _, _, cx| ws.toggle_pr_file_picker(id, cx)))
    });
    let progress = progress(id, files.len(), total, viewed, more, cx);
    panel
        .children(review)
        .child(progress)
        .children(picker)
        .child(
            div()
                .flex()
                .items_start()
                .when(!wide, |row| row.flex_col())
                .w_full()
                .min_w_0()
                .gap(px(theme::GAP_LG))
                .when(wide || picker_open, |row| {
                    row.child(
                        div()
                            .when(wide, |nav| nav.w(px(theme::PR_FILES_NAV_W)).flex_none())
                            .when(!wide, |nav| nav.w_full())
                            .child(file_list(id, files, list_scroll, cx)),
                    )
                })
                .children(shown.map(|shown| {
                    div()
                        .min_w_0()
                        .when(wide, |diff| diff.flex_1())
                        .when(!wide, |diff| diff.w_full())
                        .child(shown_diff(id, shown, ready, pr_url, wide, cx))
                })),
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
    let count = s::files_viewed(count, viewed);
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
                .id(("pr-file-path", index))
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .flex()
                .flex_col()
                .line_height(px(theme::PR_FILE_ROW_H / 2.0))
                .tooltip(tooltip::text(match &file.note {
                    Some(note) => format!("{}\n{note}", file.path),
                    None => file.path.clone(),
                }))
                .child(
                    div()
                        .truncate()
                        .font_family(theme::FONT_FAMILY_MONOSPACE)
                        .text_color(t.text_body)
                        .child(
                            file.path
                                .rsplit('/')
                                .next()
                                .unwrap_or(&file.path)
                                .to_owned(),
                        ),
                )
                .children(file.parent.as_ref().or(file.note.as_ref()).map(|detail| {
                    div()
                        .truncate()
                        .text_size(px(theme::FONT_SIZE_XS))
                        .text_color(t.text_subtle)
                        .child(detail.clone())
                })),
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
    let blob = file.blob.clone();
    let (name, index) = key;
    button_icon(key, icon, cx)
        .debug_selector(move || format!("{name}-{index}"))
        .tooltip(tip)
        .disabled(blob.is_none())
        .on_click(
            cx.listener(move |ws, _, _, cx| {
                ws.toggle_pr_file_viewed(id, &path, blob.as_deref(), cx)
            }),
        )
        .into_any_element()
}

fn shown_diff(
    id: PrDetailId,
    shown: &ShownView,
    ready: &ReadyView,
    pr_url: Option<&str>,
    wide: bool,
    cx: &Context<Workspace>,
) -> AnyElement {
    let next_unviewed = match (&ready.files, &shown.row) {
        (Some(FilesView::Ready { files, viewed, .. }), Some(row)) => {
            files.len() - viewed > usize::from(row.viewed != Viewed::Yes)
        }
        _ => false,
    };
    let can_comment = ready
        .review_comments
        .as_ref()
        .is_some_and(|comments| !comments.locked && comments.composer.is_none());
    let t = theme::current(cx);
    let body = match &shown.diff {
        FileDiff::TooLarge => too_large(id, shown, pr_url, cx),
        FileDiff::NoLines => div()
            .flex()
            .flex_wrap()
            .items_center()
            .child(note(s::file_no_lines(), cx))
            .children(open_file_on_github(shown, pr_url))
            .into_any_element(),
        FileDiff::MovedOnly => note(s::file_moved_only(), cx),
        FileDiff::Rows(rows) => super::diff_render::diff_body(id, shown, rows, &ready.threads, cx),
    };
    let rows = match &shown.diff {
        FileDiff::Rows(rows) => Some(rows),
        _ => None,
    };
    div()
        .debug_selector(|| "pr-file-diff".into())
        .flex()
        .flex_col()
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        .child(diff_header(id, shown, wide, cx))
        .children(
            shown
                .selection
                .as_ref()
                .map(|lines| selection_bar(id, lines, can_comment, ready.code_send, cx)),
        )
        .child(body)
        .children(super::diff_render::unplaced_threads(
            id,
            shown,
            &ready.threads,
            rows,
            cx,
        ))
        .children(shown.row.as_ref().map(|row| {
            let path = row.path.clone();
            let blob = row.blob.clone();
            div()
                .flex()
                .justify_end()
                .p(px(theme::PAD_SM))
                .border_t_1()
                .border_color(t.border)
                .child(
                    button_compact(
                        "pr-file-finish",
                        match (row.viewed, next_unviewed) {
                            (Viewed::Yes, _) => s::file_next_unviewed(),
                            (_, true) => s::file_finish_next(),
                            (_, false) => s::file_mark_viewed(),
                        },
                    )
                    .debug_selector(|| "pr-file-finish".into())
                    .disabled(blob.is_none() || (!next_unviewed && row.viewed == Viewed::Yes))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.finish_pr_file(id, &path, blob.as_deref(), cx)
                    })),
                )
        }))
        .into_any_element()
}

/// GitHub sent no patch: say so, and offer to read the file whole.
fn too_large(
    id: PrDetailId,
    shown: &ShownView,
    pr_url: Option<&str>,
    cx: &Context<Workspace>,
) -> AnyElement {
    let t = theme::current(cx);
    let recover = |label| {
        button_compact("pr-file-recover", label)
            .debug_selector(|| "pr-file-recover".into())
            .on_click(cx.listener(move |ws, _, _, cx| ws.recover_pr_file(id, cx)))
            .into_any_element()
    };
    let (message, retry) = match shown.recovery.as_ref() {
        None | Some(RecoveryView::Busy) => (s::file_too_large(), None),
        Some(RecoveryView::Offered) => (s::file_too_large(), Some(recover(s::file_recover()))),
        Some(RecoveryView::Reading) => (s::file_recovering(), None),
        Some(RecoveryView::Failed(failure)) => {
            let message = match failure {
                RecoverFailure::Binary => s::file_recover_binary(),
                RecoverFailure::TooLarge => s::file_recover_too_large(),
                RecoverFailure::Failed(reason) | RecoverFailure::Unavailable(reason) => {
                    s::file_recover_failed(reason)
                }
            };
            (message, failure.can_retry().then(|| recover(s::retry())))
        }
    };
    // GitHub shows the change whatever stopped reading it here.
    let github = match shown.recovery.as_ref() {
        Some(RecoveryView::Failed(_)) => open_file_on_github(shown, pr_url),
        _ => None,
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
        .children(retry)
        .children(github)
        .into_any_element()
}

/// The shown file's change on GitHub, for a file drawn here without lines.
fn open_file_on_github(shown: &ShownView, pr_url: Option<&str>) -> Option<AnyElement> {
    let url = super::recover::file_diff_url(pr_url?, &shown.path)?;
    Some(
        button_compact(
            "pr-file-open-github",
            crate::surface::strings::git::pr_open_on_github(),
        )
        .debug_selector(|| "pr-file-open-github".into())
        .on_click(move |_, _, cx| cx.open_url(&url))
        .into_any_element(),
    )
}

/// The shown file's path and place, the previous and next file, and how
/// its diff is laid out.
fn diff_header(
    id: PrDetailId,
    shown: &ShownView,
    wide: bool,
    cx: &Context<Workspace>,
) -> AnyElement {
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
        .when(wide, |header| {
            header.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(theme::FONT_FAMILY_MONOSPACE)
                    .text_size(px(theme::FONT_SIZE_SM))
                    .text_color(t.text_muted)
                    .child(SharedString::from(shown.path.clone())),
            )
        })
        .children(shown.position.map(|(at, of)| {
            div()
                .when(!wide, |place| place.flex_1())
                .text_size(px(theme::FONT_SIZE_SM))
                .text_color(t.text_subtle)
                .child(s::file_position(at + 1, of))
        }))
        .children(shown.row.as_ref().map(|row| {
            let path = row.path.clone();
            let blob = row.blob.clone();
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
            .disabled(blob.is_none())
            .on_click(cx.listener(move |ws, _, _, cx| {
                ws.toggle_pr_file_viewed(id, &path, blob.as_deref(), cx)
            }))
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
    send: SendSlot,
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
            button_compact("pr-lines-send", s::send_to_agent())
                .debug_selector(|| "pr-lines-send".into())
                .disabled(!send.enabled())
                .when(send == SendSlot::NoLane, |button| {
                    button.tooltip(s::send_no_lane())
                })
                .on_click(cx.listener(move |ws, _, _, cx| ws.open_pr_code_handoff(id, cx))),
        )
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
