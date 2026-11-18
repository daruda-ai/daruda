//! Choosing lines of the shown file: by click, by a thread's place, and
//! keeping them across a reread.

use super::*;

impl FilesState {
    /// Show `target`'s file and select its lines once the file is shown at
    /// a known commit.
    pub(in super::super) fn go_to_lines(&mut self, target: super::super::view::ThreadTarget) {
        self.restore = None;
        let path = target.path.clone();
        self.pending_lines = Some(target);
        self.show(&path);
        self.select_pending();
    }

    /// Select the pending lines if their file is shown at a known commit.
    pub(super) fn select_pending(&mut self) {
        let Some(target) = self.pending_lines.as_ref() else {
            return;
        };
        let Some(shown) = self
            .shown
            .as_ref()
            .filter(|shown| shown.path == target.path)
        else {
            return;
        };
        let Some(revision) = shown.revision.clone() else {
            return;
        };
        let target = self.pending_lines.take().expect("checked");
        self.selection = Some(LineRange {
            path: target.path,
            side: target.side,
            start: target.start,
            end: target.end,
            revision,
        });
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
        let file = self.files.iter().find(|file| file.path == range.path)?;
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
        let held = self.selection.as_ref()?;
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
        self.selection.as_ref() == Some(range)
            && self.revision.as_deref() == Some(range.revision.as_str())
    }

    pub(in crate::workspace) fn selection(&self) -> Option<&LineRange> {
        self.selection.as_ref()
    }

    /// The selection's reference, and below it the selected lines when the
    /// shown file is the one they are in.
    pub(super) fn selection_copy(&self) -> Option<String> {
        let held = self.selection.as_ref()?;
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
