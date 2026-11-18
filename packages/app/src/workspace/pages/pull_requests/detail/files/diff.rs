//! A file's diff as rows: unified or side by side, folded or whole.

use super::*;

/// A file's diff rows, unified and paired side by side, shared by the
/// state and each frame's view. Equal only to itself: comparing two diffs
/// row by row is not worth a frame.
#[derive(Clone)]
pub(in crate::workspace) struct DiffRows {
    unified: Arc<Vec<VisualRow>>,
    split: Arc<Vec<SplitRow>>,
}

impl DiffRows {
    pub(super) fn new(rows: Vec<VisualRow>) -> Self {
        let split = split_rows(&rows);
        Self {
            unified: Arc::new(rows),
            split: Arc::new(split),
        }
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
