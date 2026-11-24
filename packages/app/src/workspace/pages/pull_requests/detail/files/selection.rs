//! Choosing lines of the shown file: by click, by a thread's place, and
//! keeping them across a reread.

use super::*;

impl FilesState {
    /// Show the latest available version without selecting a stale draft's
    /// lines; a file on a page not read yet is shown when it lands.
    pub(in super::super) fn show_listed(&mut self, path: &str) {
        self.restore = None;
        self.clear_selection();
        self.show(path);
    }

    pub(in super::super) fn shown_path(&self) -> Option<&str> {
        self.shown.as_ref().map(|shown| shown.path.as_str())
    }

    pub(in super::super) fn shown_at(&self, head: &str) -> bool {
        self.revision.as_deref() == Some(head)
            && self
                .shown
                .as_ref()
                .is_some_and(|shown| shown.revision.as_deref() == Some(head))
    }

    /// Show `target`'s file and select its lines once the file is shown at
    /// a known commit.
    pub(in super::super) fn go_to_lines(&mut self, target: super::super::view::ThreadTarget) {
        self.restore = None;
        let path = target.path.clone();
        self.set_unchanged(Unchanged::Shown);
        self.pending_lines = Some(target);
        self.show(&path);
        self.select_pending();
    }

    /// Select the pending lines if their file is shown at a known commit.
    pub(super) fn select_pending(&mut self) {
        if self.replacing {
            return;
        }
        let Some(target) = self.pending_lines.as_ref() else {
            return;
        };
        let Some(shown) = self
            .shown
            .as_mut()
            .filter(|shown| shown.path == target.path)
        else {
            return;
        };
        let Some(revision) = shown.revision.clone() else {
            return;
        };
        let target = self.pending_lines.take().expect("checked");
        // Lines a thread was read on at another commit name other code here.
        if target.revision != revision {
            self.selection = None;
            return;
        }
        if let FileDiff::Rows(rows) = &shown.diff
            && let Some(index) = rows.row_index(target.side, target.end, self.mode)
        {
            if !(shown.start..shown.limit).contains(&index) {
                // Draw a step around the lines, not every row before them.
                shown.start = index.saturating_sub(DIFF_ROWS_STEP / 2);
                shown.limit = shown.start + DIFF_ROWS_STEP;
            }
        } else {
            self.selection = None;
            return;
        }
        self.selection = Some(LineRange {
            path: target.path,
            side: target.side,
            start: target.start,
            end: target.end,
            revision,
        });
    }

    /// Whether the chosen thread can be placed now. A later page or detail
    /// read can still supply a missing file or revision.
    pub(in super::super) fn thread_placement(
        &self,
        target: &super::super::view::ThreadTarget,
    ) -> Option<bool> {
        if self.replacing {
            return (!matches!(self.paging, Paging::Reading { .. })).then_some(false);
        }
        if let Some(shown) = self
            .shown
            .as_ref()
            .filter(|shown| shown.path == target.path)
        {
            let revision = shown.revision.as_ref()?;
            let drawn = shown.start..shown.limit;
            return Some(
                *revision == target.revision
                    && matches!(&shown.diff, FileDiff::Rows(rows)
                        if rows
                            .row_index(target.side, target.end, self.mode)
                            .is_some_and(|ix| drawn.contains(&ix))),
            );
        }
        (!matches!(self.paging, Paging::Reading { .. })).then_some(false)
    }

    /// Lines selected at another commit than `revision` no longer point at
    /// the code shown.
    pub(super) fn keep_selection_at(&mut self, revision: Option<String>) {
        if let Some(revision) = revision
            && self
                .selection
                .as_ref()
                .is_some_and(|held| held.revision != revision)
        {
            self.selection = None;
        }
    }

    /// Select `line` on `side` of the shown file, or stretch the selection
    /// to it. Nothing while the commit its rows were read at is unknown.
    pub(super) fn select_line(&mut self, side: Side, line: u32, extend: bool) {
        self.pending_lines = None;
        let Some((shown, Some(revision))) = self
            .shown
            .as_ref()
            .map(|shown| (shown, shown.revision.as_ref()))
        else {
            return;
        };
        self.restore = None;
        if self.chosen.as_deref() != Some(shown.path.as_str()) {
            self.chosen = Some(shown.path.clone());
        }
        self.selection = Some(select(
            self.selection.as_ref(),
            &shown.path,
            side,
            line,
            revision,
            extend,
        ));
    }

    pub(super) fn clear_selection(&mut self) {
        self.selection = None;
        self.pending_lines = None;
    }

    /// The head commit the listing was read at, once known.
    pub(in crate::workspace) fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }

    /// Whether `range` can anchor a review comment in the listing read:
    /// its file listed with a patch, every line in it within one hunk, at
    /// the listing's commit. `None` when the listing cannot say.
    pub(in crate::workspace) fn anchors(&self, range: &LineRange) -> Option<bool> {
        if self.revision.as_deref() != Some(range.revision.as_str()) {
            return None;
        }
        let Some(file) = self.files.iter().find(|file| file.path == range.path) else {
            // A partial listing cannot rule out a file on a later page.
            return (self.complete() && !self.capped()).then_some(false);
        };
        let rows = unfolded_rows(file)?;
        Some(super::super::diff_layout::anchored_rows(&rows, range).is_some())
    }

    /// The text of `range`'s lines, from the unfolded diff at its commit,
    /// when every line is there within one hunk: what a suggestion starts
    /// from. `None` otherwise, so it starts empty rather than wrong.
    pub(in crate::workspace) fn lines_text(&self, range: &LineRange) -> Option<String> {
        if self.revision.as_deref() != Some(range.revision.as_str()) {
            return None;
        }
        let file = self.files.iter().find(|file| file.path == range.path)?;
        let rows = unfolded_rows(file)?;
        let lines = super::super::diff_layout::anchored_rows(&rows, range)?;
        Some(
            lines
                .iter()
                .map(|row| row.content.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// The selection and the code the diff shows of it, read from the
    /// unfolded diff so folded lines are not skipped, and whether every
    /// selected line is there. `None` without a selection made in this
    /// listing whose file has a patch.
    pub(in super::super) fn selection_code(&self) -> Option<(LineRange, String, bool)> {
        let held = self.selection()?;
        if self.revision.as_deref() != Some(held.revision.as_str()) {
            return None;
        }
        let file = self.files.iter().find(|file| file.path == held.path)?;
        let rows = unfolded_rows(file)?;
        let lines: Vec<&str> = rows
            .iter()
            .filter(|row| held.holds(row))
            .map(|row| row.content.as_str())
            .collect();
        if lines.is_empty() {
            return None;
        }
        let complete = lines.len() as u64 == u64::from(held.end - held.start) + 1;
        Some((held.clone(), lines.join("\n"), complete))
    }

    /// Whether `range` is still the selection, in the listing read.
    pub(in super::super) fn selection_current(&self, range: &LineRange) -> bool {
        self.selection() == Some(range) && self.revision.as_deref() == Some(range.revision.as_str())
    }

    /// A selection remains associated with its file when navigating away.
    /// Only the file on screen can supply a comment, clipboard or handoff action.
    pub(in crate::workspace) fn selection(&self) -> Option<&LineRange> {
        let shown = self.shown.as_ref()?;
        self.selection
            .as_ref()
            .filter(|held| held.path == shown.path)
    }

    /// The selection's reference, and below it the selected lines when the
    /// shown file is the one they are in.
    pub(super) fn selection_copy(&self) -> Option<String> {
        let held = self.selection()?;
        let lines = self
            .shown
            .as_ref()
            .filter(|shown| shown.path == held.path)
            .and_then(|shown| match &shown.diff {
                FileDiff::Rows(rows) => Some(selected_text(rows.rows(), held)),
                _ => None,
            })
            .filter(|text| !text.is_empty());
        Some(match lines {
            Some(lines) => format!("{}\n{lines}", held.reference()),
            None => held.reference(),
        })
    }
}
