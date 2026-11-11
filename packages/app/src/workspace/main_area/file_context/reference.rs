//! Immutable source selections and their prompt representation.

use std::{ops::Range, path::PathBuf};

use super::super::file_view_pane::{DiffSource, VisualRow, VisualRowKind};

pub(super) use super::super::pane_menu::SEND_SELECTION_LIMIT as EXCERPT_LIMIT;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SourceRow {
    pub kind: VisualRowKind,
    pub old: String,
    pub new: String,
}

impl From<&VisualRow> for SourceRow {
    fn from(row: &VisualRow) -> Self {
        Self {
            kind: row.kind,
            old: row.line_no_left.clone(),
            new: row.line_no_right.clone(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum SelectionSource {
    Raw { unsaved: bool },
    Diff(Vec<SourceRow>),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct SelectedSource {
    pub text: String,
    pub range: Range<usize>,
    pub source: SelectionSource,
}

impl SelectedSource {
    pub fn excerpt(&self) -> &str {
        self.text.get(self.range.clone()).unwrap_or_default()
    }

    pub fn line_range(&self) -> Range<usize> {
        let start = self.text[..self.range.start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        let end = self.range.end.saturating_sub(1).max(self.range.start);
        let end = self.text.as_bytes()[..end]
            .iter()
            .copied()
            .filter(|b| *b == b'\n')
            .count();
        start..end + 1
    }

    pub fn has_code(&self) -> bool {
        if self.range.is_empty() || self.text.get(self.range.clone()).is_none() {
            return false;
        }
        match &self.source {
            SelectionSource::Raw { .. } => true,
            SelectionSource::Diff(rows) => self.line_range().any(|i| {
                rows.get(i)
                    .is_some_and(|r| r.kind != VisualRowKind::HunkHeader)
            }),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(in crate::workspace) struct FileReference {
    pub(super) path: PathBuf,
    pub(super) source: DiffSource,
    pub(super) selected: Option<SelectedSource>,
}

pub(super) fn inline_code(value: &str) -> String {
    let fence = "`".repeat(value.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1);
    if value.starts_with('`') || value.ends_with('`') {
        format!("{fence} {value} {fence}")
    } else {
        format!("{fence}{value}{fence}")
    }
}

fn line_label(lines: Range<usize>) -> String {
    if lines.len() == 1 {
        (lines.start + 1).to_string()
    } else {
        format!("{}-{}", lines.start + 1, lines.end)
    }
}

fn fenced(text: &str, language: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{language}\n{text}\n{fence}\n")
}

impl FileReference {
    pub(super) fn label(&self, path: &str) -> String {
        match self.selected.as_ref() {
            Some(selected) => match &selected.source {
                SelectionSource::Raw { .. } => {
                    format!("{path}:{}", line_label(selected.line_range()))
                }
                SelectionSource::Diff(rows) => {
                    let mut lines = Vec::new();
                    for (prefix, old) in [("−", true), ("+", false)] {
                        let mut selected_lines = selected
                            .line_range()
                            .filter_map(|i| rows.get(i))
                            .map(|row| {
                                if old {
                                    row.old.as_str()
                                } else {
                                    row.new.as_str()
                                }
                            })
                            .filter(|line| !line.is_empty());
                        if let Some(first) = selected_lines.next() {
                            let last = selected_lines.next_back().unwrap_or(first);
                            lines.push(if first == last {
                                format!("{prefix}{first}")
                            } else {
                                format!("{prefix}{first}-{last}")
                            });
                        }
                    }
                    format!("{path} ({})", lines.join(","))
                }
            },
            None => path.to_owned(),
        }
    }

    pub(super) fn format(
        &self,
        path: &str,
        shell: Option<daruda_core::shell::quote::Shell>,
    ) -> String {
        if let Some(shell) = shell {
            let plain = match &self.selected {
                None => path.to_string(),
                Some(_) => self.format(path, None),
            };
            return daruda_core::shell::quote::quote_str(&plain, shell);
        }
        let quoted = inline_code;
        let Some(selected) = &self.selected else {
            return quoted(path);
        };
        match &selected.source {
            SelectionSource::Raw { unsaved } => {
                let reference = quoted(&format!("{path}:{}", line_label(selected.line_range())));
                let label = if *unsaved {
                    crate::surface::strings::file_viewer::input_unsaved_selection()
                } else if self.source.is_editable() {
                    crate::surface::strings::file_viewer::input_selection_snapshot()
                } else {
                    let source = match &self.source {
                        DiffSource::Index => "index",
                        DiffSource::Range { to, .. } => to.as_str(),
                        DiffSource::WorkingTree => "worktree",
                    };
                    crate::surface::strings::file_viewer::input_versioned_selection(source)
                };
                format!("{reference}\n{}\n{}", label, fenced(selected.excerpt(), ""))
            }
            SelectionSource::Diff(rows) => {
                let provenance = match &self.source {
                    DiffSource::WorkingTree => "index -> working tree".to_string(),
                    DiffSource::Index => "HEAD -> index".to_string(),
                    DiffSource::Range {
                        from, to, old_path, ..
                    } => match old_path {
                        Some(old) => crate::surface::strings::file_viewer::input_diff_rename(
                            from,
                            to,
                            old.display(),
                        ),
                        None => format!("{from} -> {to}"),
                    },
                };
                let mut old = Vec::new();
                let mut new = Vec::new();
                let mut excerpt = Vec::new();
                let lines: Vec<_> = selected.text.split('\n').collect();
                for i in selected.line_range() {
                    let Some(row) = rows.get(i) else {
                        continue;
                    };
                    if row.kind == VisualRowKind::HunkHeader {
                        continue;
                    }
                    if !row.old.is_empty() {
                        old.push(row.old.as_str());
                    }
                    if !row.new.is_empty() {
                        new.push(row.new.as_str());
                    }
                    let marker = match row.kind {
                        VisualRowKind::Added => '+',
                        VisualRowKind::Removed => '-',
                        _ => ' ',
                    };
                    excerpt.push(format!(
                        "{marker}{}",
                        lines.get(i).copied().unwrap_or_default()
                    ));
                }
                format!(
                    "{}\n{}",
                    crate::surface::strings::file_viewer::input_diff_selection(
                        quoted(path),
                        provenance,
                        old.join(", "),
                        new.join(", ")
                    ),
                    fenced(&excerpt.join("\n"), "diff")
                )
            }
        }
    }
}

#[cfg(test)]
mod tests;
