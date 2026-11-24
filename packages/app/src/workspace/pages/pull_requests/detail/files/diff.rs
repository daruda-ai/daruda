//! A file's diff as rows: unified or side by side, folded or whole.

use super::super::diff_layout::line_on;
use super::*;

#[derive(Clone, Copy)]
struct LinePosition {
    number: u32,
    unified: usize,
    split: usize,
}

/// A file's diff rows, unified and paired side by side, shared by the
/// state and each frame's view. Equal only to itself: comparing two diffs
/// row by row is not worth a frame.
#[derive(Clone)]
pub(in crate::workspace) struct DiffRows {
    unified: Arc<Vec<VisualRow>>,
    split: Arc<Vec<SplitRow>>,
    // Each side is ordered by source line. Its positions serve both layouts.
    positions: Arc<[Vec<LinePosition>; 2]>,
}

impl DiffRows {
    pub(super) fn new(rows: Vec<VisualRow>) -> Self {
        let split = split_rows(&rows);
        let mut positions = [Vec::new(), Vec::new()];
        for (index, row) in split.iter().enumerate() {
            if let SplitRow::Pair { old, new } = row {
                for (side, cell) in [(Side::Old, old), (Side::New, new)] {
                    if let Some(cell) = cell
                        && let Some(number) = line_on(&rows[cell.row], side)
                    {
                        positions[side as usize].push(LinePosition {
                            number,
                            unified: cell.row,
                            split: index,
                        });
                    }
                }
            }
        }
        Self {
            positions: Arc::new(positions),
            unified: Arc::new(rows),
            split: Arc::new(split),
        }
    }

    /// The displayed row for an exact source line on the requested side.
    pub(in super::super) fn row_index(
        &self,
        side: Side,
        line: u32,
        mode: DiffMode,
    ) -> Option<usize> {
        let positions = &self.positions[side as usize];
        let index = positions
            .binary_search_by_key(&line, |pos| pos.number)
            .ok()?;
        Some(match mode {
            DiffMode::Unified => positions[index].unified,
            DiffMode::Split => positions[index].split,
        })
    }

    pub(in crate::workspace) fn rows(&self) -> &[VisualRow] {
        &self.unified
    }

    pub(in crate::workspace) fn split(&self) -> &[SplitRow] {
        &self.split
    }
}

impl PartialEq for DiffRows {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.unified, &other.unified)
    }
}

impl Eq for DiffRows {}

impl std::fmt::Debug for DiffRows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DiffRows({})", self.unified.len())
    }
}

/// How a diff is laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::workspace) enum DiffMode {
    #[default]
    Unified,
    /// Old on the left, new on the right.
    Split,
}

/// Whether a diff shows the lines around its changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::workspace) enum Unchanged {
    #[default]
    Shown,
    /// Only changed lines, under their hunk headers.
    Folded,
}

/// What the chosen file shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum FileDiff {
    Rows(DiffRows),
    /// Lines changed, but GitHub sent no patch: too large to diff.
    TooLarge,
    /// No line changed: a binary file, or a mode change.
    NoLines,
    /// Moved or copied without an edit.
    MovedOnly,
}

impl FileDiff {
    pub(super) fn of(file: &PrFile, unchanged: Unchanged) -> Self {
        let moved = matches!(
            file.change,
            FileChange::Renamed { .. } | FileChange::Copied { .. }
        );
        let lines = file.additions + file.deletions;
        let rows = file.patch.as_deref().map(|patch| {
            let no_newline = crate::surface::strings::file_viewer::no_newline();
            let fold = unchanged == Unchanged::Folded;
            build_diff_rows(&parse_diff_hunks(patch), fold, &no_newline)
        });
        match rows {
            Some(rows) if !rows.is_empty() => Self::Rows(DiffRows::new(rows)),
            _ if moved && lines == 0 => Self::MovedOnly,
            None if lines > 0 => Self::TooLarge,
            _ => Self::NoLines,
        }
    }
}

/// `file`'s diff rows with every line its patch holds, folding aside.
pub(super) fn unfolded_rows(file: &PrFile) -> Option<Vec<VisualRow>> {
    let patch = file.patch.as_deref()?;
    let no_newline = crate::surface::strings::file_viewer::no_newline();
    Some(build_diff_rows(
        &parse_diff_hunks(patch),
        false,
        &no_newline,
    ))
}
