//! How a file's diff is laid out and pointed into, GPUI-free: unified rows
//! paired side by side, and the line a row stands for on the old or new
//! side, which a selection of lines is made of.

use daruda_content::diff::{VisualRow, VisualRowKind};

/// Which version of the file a line number counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::workspace) enum Side {
    /// The base: removed and unchanged lines.
    Old,
    /// The head: added and unchanged lines.
    New,
}

/// The line `row` holds on `side`, if any: a removed line is on the old
/// side only, an added line on the new side only, an unchanged line on both
/// under its own number on each.
pub(in crate::workspace) fn line_on(row: &VisualRow, side: Side) -> Option<u32> {
    let number = |text: &str| text.trim().parse::<u32>().ok();
    match (row.kind, side) {
        (VisualRowKind::Removed | VisualRowKind::Context, Side::Old) => number(&row.line_no_left),
        (VisualRowKind::Added | VisualRowKind::Context, Side::New) => number(&row.line_no_right),
        _ => None,
    }
}

/// A range of lines on one side of one file, at one revision of the pull
/// request: what a comment or an agent is pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct LineRange {
    pub path: String,
    pub side: Side,
    /// First and last line, inclusive, first ≤ last.
    pub start: u32,
    pub end: u32,
    /// The head commit the lines were read at.
    pub revision: String,
}

impl LineRange {
    /// Whether `row` falls in the range.
    pub(in crate::workspace) fn holds(&self, row: &VisualRow) -> bool {
        line_on(row, self.side).is_some_and(|line| (self.start..=self.end).contains(&line))
    }

    /// `path:12-15 (new)`, without the commit.
    pub(in crate::workspace) fn place(&self) -> String {
        let lines = if self.start == self.end {
            self.start.to_string()
        } else {
            format!("{}-{}", self.start, self.end)
        };
        let side = match self.side {
            Side::Old => "old",
            Side::New => "new",
        };
        format!("{}:{lines} ({side})", self.path)
    }

    /// `path:12-15 (new) @ 4f9c2e1`, as copied for a person or an agent.
    pub(in crate::workspace) fn reference(&self) -> String {
        let revision: String = self.revision.chars().take(7).collect();
        format!("{} @ {revision}", self.place())
    }
}

/// The lines `range` covers, in order, when every one of them is in the
/// diff on its side within one hunk: the only ranges a review comment can
/// be anchored to. `rows` must be unfolded: folding drops unchanged lines.
pub(in crate::workspace) fn anchored_rows<'a>(
    rows: &'a [VisualRow],
    range: &LineRange,
) -> Option<Vec<&'a VisualRow>> {
    let mut found: Vec<&VisualRow> = Vec::new();
    let mut hunk = 0;
    let mut hunk_of_first = None;
    for row in rows {
        if row.kind == VisualRowKind::HunkHeader {
            hunk += 1;
            continue;
        }
        if range.holds(row) {
            if *hunk_of_first.get_or_insert(hunk) != hunk {
                return None;
            }
            found.push(row);
        }
    }
    let wanted = (range.end - range.start + 1) as usize;
    (found.len() == wanted).then_some(found)
}

/// The text of the lines in `range` among `rows`, one per line, in order:
/// what copying a selection puts below its reference.
pub(in crate::workspace) fn selected_text(rows: &[VisualRow], range: &LineRange) -> String {
    rows.iter()
        .filter(|row| range.holds(row))
        .map(|row| row.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Select `line` on `side` of `path`; with `extend`, stretch an existing
/// selection on the same file and side to it instead.
pub(in crate::workspace) fn select(
    held: Option<&LineRange>,
    path: &str,
    side: Side,
    line: u32,
    revision: &str,
    extend: bool,
) -> LineRange {
    match held {
        Some(held)
            if extend && held.path == path && held.side == side && held.revision == revision =>
        {
            LineRange {
                start: held.start.min(line),
                end: held.end.max(line),
                ..held.clone()
            }
        }
        _ => LineRange {
            path: path.to_owned(),
            side,
            start: line,
            end: line,
            revision: revision.to_owned(),
        },
    }
}

/// One side of a side-by-side row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct Cell {
    /// The unified row it came from, which carries its number and text.
    pub row: usize,
}

/// A row of the side-by-side layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum SplitRow {
    /// A hunk header or a no-newline marker, across both sides.
    Banner(usize),
    /// The old line on the left, the new on the right; either may be blank.
    Pair {
        old: Option<Cell>,
        new: Option<Cell>,
    },
}

/// Pair unified `rows` side by side: an unchanged line on both sides, a run
/// of removed lines beside the run of added lines that follows it, line for
/// line, the longer run's rest beside blanks.
pub(in crate::workspace) fn split_rows(rows: &[VisualRow]) -> Vec<SplitRow> {
    let mut out = Vec::new();
    let mut removed: Vec<usize> = Vec::new();
    let mut added: Vec<usize> = Vec::new();
    let flush = |out: &mut Vec<SplitRow>, removed: &mut Vec<usize>, added: &mut Vec<usize>| {
        let pairs = removed.len().max(added.len());
        for i in 0..pairs {
            out.push(SplitRow::Pair {
                old: removed.get(i).map(|&row| Cell { row }),
                new: added.get(i).map(|&row| Cell { row }),
            });
        }
        removed.clear();
        added.clear();
    };
    for (index, row) in rows.iter().enumerate() {
        match row.kind {
            VisualRowKind::Removed => {
                // A removal after additions starts a new run.
                if !added.is_empty() {
                    flush(&mut out, &mut removed, &mut added);
                }
                removed.push(index);
            }
            VisualRowKind::Added => added.push(index),
            VisualRowKind::Context | VisualRowKind::Plain => {
                flush(&mut out, &mut removed, &mut added);
                out.push(SplitRow::Pair {
                    old: Some(Cell { row: index }),
                    new: Some(Cell { row: index }),
                });
            }
            VisualRowKind::HunkHeader | VisualRowKind::NoNewline => {
                flush(&mut out, &mut removed, &mut added);
                out.push(SplitRow::Banner(index));
            }
        }
    }
    flush(&mut out, &mut removed, &mut added);
    out
}

#[cfg(test)]
mod tests;
