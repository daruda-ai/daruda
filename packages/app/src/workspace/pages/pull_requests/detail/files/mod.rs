//! The detail's Files tab: the files the pull request changes, read page by
//! page from when the tab is first shown, and the unified diff of the one
//! chosen. Reading stops at a transfer budget and resumes on request; a
//! page that fails keeps what came before it. The diff's rows are built
//! once per choice and drawn a bounded number at a time.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use daruda_content::diff::{VisualRow, build_diff_rows, parse_diff_hunks};
use daruda_github::{FileChange, GhError, PrFile, PrFilesPage};
use gpui::Context;

use super::diff_layout::{LineRange, Side, SplitRow, select, selected_text, split_rows};
use super::progress::{PlaceFile, Viewed};
use super::read_slot::ReadTicket;
use super::recover::{RecoverError, RecoverFailure, RecoverTarget, Recovered, recover_patch};
use super::{PrDetailId, PrDetailState};
use crate::gh_host::GhHost;
use crate::surface::strings::pull_requests as s;
use crate::workspace::Workspace;
use crate::workspace::spawn_helpers::spawn_bg_work_and_mutate;

/// Patch text read before pages stop being read on their own: a pull
/// request this large is read further only when asked.
const AUTO_READ_BYTES: usize = 4 * 1024 * 1024;
/// Diff rows drawn at first, and added by each "show more".
pub(in crate::workspace) const DIFF_ROWS_STEP: usize = 1_000;

/// The chosen file, by path, its diff, and which of its rows are drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    path: String,
    diff: FileDiff,
    /// Rows `start..limit` are drawn. A jump far into a long diff moves the
    /// window there instead of drawing every row before it.
    start: usize,
    limit: usize,
    /// The head commit its rows were read at, once known: what lines
    /// selected in them are pinned to. An earlier listing's rows stay up
    /// through a refresh under their own commit.
    revision: Option<String>,
}

impl Shown {
    fn draw_from_top(&mut self) {
        self.start = 0;
        self.limit = DIFF_ROWS_STEP;
    }

    /// Have row `index` drawn: where it is not, a step of rows around it
    /// is, rather than every row before it.
    fn draw_around(&mut self, index: usize) {
        if !(self.start..self.limit).contains(&index) {
            self.start = index.saturating_sub(DIFF_ROWS_STEP / 2);
            self.limit = self.start + DIFF_ROWS_STEP;
        }
    }
}

/// Where reading the pages stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Paging {
    /// Page `page` is being read under `ticket`.
    Reading { ticket: ReadTicket, page: u32 },
    /// Stopped at the transfer budget; page `next` is read on request.
    Paused { next: u32 },
    /// Page `page` failed; what came before stays listed.
    Failed { page: u32, reason: String },
    /// Every page GitHub lists is read.
    Done,
}

/// What a landed page asks the caller to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Landed {
    /// Not the read this tab waits on.
    Stale,
    /// Nothing more to send now.
    Settled,
    /// Read page `page` under `ticket`.
    Next(ReadTicket, u32),
}

/// The Files tab's state: what is listed, how reading stands, and the file
/// chosen.
#[derive(Debug)]
pub(in crate::workspace) struct FilesState {
    files: Vec<PrFile>,
    /// Duplicate basenames can acquire a new sibling when another page lands.
    parents: OnceCell<HashMap<String, String>>,
    /// The files listed are an earlier read's, until this read's first page
    /// lands: a refresh keeps them up meanwhile.
    replacing: bool,
    paging: Paging,
    /// GitHub's cap stopped the listing.
    capped: bool,
    patch_bytes: usize,
    /// Patch text after which pages stop being read on their own; lifted
    /// when the person asks for the rest.
    budget: usize,
    /// The file chosen, kept while pages arrive even before it is listed
    /// again after a refresh.
    chosen: Option<String>,
    /// Its rows, as last built from a page that listed it.
    shown: Option<Shown>,
    picker_open: bool,
    mode: DiffMode,
    unchanged: Unchanged,
    /// The head commit the listing was read at, once known. GitHub's file
    /// listing names no commit, so it is the pull request's head as last
    /// read, taken when the first page lands or when that read lands later.
    revision: Option<String>,
    selection: Option<LineRange>,
    /// Coming back to where the detail last closed, until the file it
    /// showed is shown again or the listing turns out not to hold it.
    restore: Option<Restore>,
    /// A file GitHub sent no patch for, being read whole or failed to.
    recovering: Option<Recovering>,
    /// Failures are pinned to content's revision and survive visits to other files.
    recovery_failures: HashMap<String, (String, RecoverFailure)>,
    /// Lines to select once their file is shown at a known commit: where a
    /// review thread sits.
    pending_lines: Option<super::view::ThreadTarget>,
    /// The merge base a file read whole was diffed from, with the head it
    /// was found for: the same for every file of one listing.
    merge_base: Option<(String, String)>,
    /// The branch the pull request merged into when last read. Files read
    /// whole, and why they could not be, are against it.
    base: Option<String>,
}

/// What reading a file whole is asked to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecoverStart {
    pub path: String,
    pub change: FileChange,
    pub lines: u32,
    pub head: String,
    pub merge_base: Option<String>,
}

/// Reading one file's diff whole, at one listing's commit.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recovering {
    path: String,
    revision: String,
    ticket: ReadTicket,
}

/// What the shown file offers when GitHub sent no patch for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum RecoveryView {
    /// Read it whole on request.
    Offered,
    /// Another file is being read whole; one at a time.
    Busy,
    Reading,
    Failed(RecoverFailure),
}

/// Where a reopened detail comes back to.
#[derive(Debug, Clone, PartialEq)]
struct Restore {
    path: String,
    /// The content the file had then: only the same content is scrolled to
    /// where it was.
    blob: Option<String>,
    offset: (f32, f32),
}

/// The file come back to, by its place in the listing, and the detail's
/// scroll offset when its content is unchanged.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Restored {
    pub index: usize,
    pub offset: Option<(f32, f32)>,
}

impl FilesState {
    pub(super) fn reading(ticket: ReadTicket) -> Self {
        Self {
            files: Vec::new(),
            parents: OnceCell::new(),
            replacing: false,
            paging: Paging::Reading { ticket, page: 1 },
            capped: false,
            patch_bytes: 0,
            budget: AUTO_READ_BYTES,
            chosen: None,
            shown: None,
            picker_open: false,
            mode: DiffMode::default(),
            unchanged: Unchanged::default(),
            revision: None,
            selection: None,
            restore: None,
            recovering: None,
            recovery_failures: HashMap::new(),
            pending_lines: None,
            merge_base: None,
            base: None,
        }
    }

    /// Read again from the first page; the listing and the chosen file stay
    /// up until it lands.
    fn reread(&mut self, ticket: ReadTicket) {
        // A refreshed listing can reflect a repaired login or gh installation.
        self.recovery_failures.retain(|_, (_, failure)| {
            matches!(failure, RecoverFailure::Binary | RecoverFailure::TooLarge)
        });
        self.replacing = !self.files.is_empty();
        self.paging = Paging::Reading { ticket, page: 1 };
        self.budget = AUTO_READ_BYTES;
    }

    /// Whether a read is worth starting when the tab is shown: nothing is
    /// listed, and nothing is being read, after a failure or before any.
    pub(super) fn wants_read(&self) -> bool {
        self.files.is_empty() && matches!(self.paging, Paging::Failed { .. })
    }

    /// The page being read, for a test to answer it late.
    #[cfg(test)]
    pub(in crate::workspace) fn waiting(&self) -> Option<(ReadTicket, u32)> {
        match self.paging {
            Paging::Reading { ticket, page } => Some((ticket, page)),
            _ => None,
        }
    }

    /// Whether every page GitHub lists is in, as far as it lists them.
    pub(super) fn complete(&self) -> bool {
        self.paging == Paging::Done && !self.replacing
    }

    pub(super) fn len(&self) -> usize {
        self.files.len()
    }

    pub(super) fn capped(&self) -> bool {
        self.capped
    }

    /// Take page `page` if it is the one waited on, and say what follows.
    /// `next` issues the ticket of a further read.
    fn land(
        &mut self,
        ticket: ReadTicket,
        page: u32,
        answer: Result<PrFilesPage, String>,
        head: Option<&str>,
        next: impl FnOnce() -> ReadTicket,
    ) -> Landed {
        if self.paging != (Paging::Reading { ticket, page }) {
            return Landed::Stale;
        }
        let read = match answer {
            Ok(read) => read,
            Err(reason) => {
                self.paging = Paging::Failed { page, reason };
                return Landed::Settled;
            }
        };
        if self.replacing {
            self.files.clear();
            self.patch_bytes = 0;
            self.replacing = false;
        }
        if page == 1 {
            self.set_revision(head);
            // Files read whole belonged to the listing replaced.
            self.recovering = None;
        }
        self.patch_bytes += read
            .files
            .iter()
            .filter_map(|file| file.patch.as_ref())
            .map(String::len)
            .sum::<usize>();
        let first = self.files.len();
        self.files.extend(read.files);
        self.parents.take();
        self.capped = read.capped;
        let last = read.next.is_none() || self.patch_bytes >= self.budget;
        self.follow_choice(first, last);
        match read.next {
            Some(page) if self.patch_bytes < self.budget => {
                let ticket = next();
                self.paging = Paging::Reading { ticket, page };
                Landed::Next(ticket, page)
            }
            Some(page) => {
                self.paging = Paging::Paused { next: page };
                Landed::Settled
            }
            None => {
                self.paging = Paging::Done;
                Landed::Settled
            }
        }
    }

    /// Read on from where reading stopped or failed. `None` when it did not.
    fn resume(&mut self, ticket: ReadTicket) -> Option<u32> {
        let page = match self.paging {
            // Asked for the rest: read it all, past the budget.
            Paging::Paused { next } => {
                self.budget = usize::MAX;
                next
            }
            Paging::Failed { page, .. } => page,
            Paging::Reading { .. } | Paging::Done => return None,
        };
        self.paging = Paging::Reading { ticket, page };
        Some(page)
    }

    /// Choose `path` and show its diff, if it is listed. The file already
    /// shown keeps its rows and how many are drawn.
    fn show(&mut self, path: &str) {
        self.picker_open = false;
        // Lines waited for in another file are given up.
        if self
            .pending_lines
            .as_ref()
            .is_some_and(|held| held.path != path)
        {
            self.pending_lines = None;
        }
        if !self.files.iter().any(|file| file.path == path) {
            // Not listed yet: while pages are still read, nothing is drawn
            // until its page lands, rather than another file standing in
            // for it. With no page on its way, what is shown stays.
            if matches!(self.paging, Paging::Reading { .. }) {
                self.chosen = Some(path.to_owned());
                self.shown = None;
            }
            return;
        }
        self.chosen = Some(path.to_owned());
        if self.shown.as_ref().is_some_and(|shown| shown.path == path) {
            return;
        }
        self.build_shown(path, (0, DIFF_ROWS_STEP));
    }

    /// The drawn rows of the shown file, to keep through a rebuild of it.
    fn drawn(&self) -> (usize, usize) {
        self.shown
            .as_ref()
            .map_or((0, DIFF_ROWS_STEP), |shown| (shown.start, shown.limit))
    }

    fn build_shown(&mut self, path: &str, (start, limit): (usize, usize)) {
        if let Some(file) = self.files.iter().find(|file| file.path == path) {
            self.shown = Some(Shown {
                path: file.path.clone(),
                diff: FileDiff::of(file, self.unchanged),
                start,
                limit,
                revision: self.revision.clone(),
            });
            // Rows rebuilt from another listing may be fewer than a window
            // moved into the old ones.
            if let Some(shown) = self.shown.as_mut()
                && shown.start >= shown.diff.row_count(self.mode)
            {
                shown.draw_from_top();
            }
            self.keep_selection_at(self.revision.clone());
            self.select_pending();
        }
    }

    /// Unified or split, folded or not, a diff's rows are numbered anew: a
    /// window moved into the old rows names other rows now. It goes back to
    /// the top, then onto the selected lines when they lie outside it.
    fn rewindow(&mut self) {
        let mode = self.mode;
        let Some(shown) = self.shown.as_mut() else {
            return;
        };
        if shown.start > 0 {
            shown.draw_from_top();
        }
        if let Some(lines) = self.selection.as_ref().filter(|l| l.path == shown.path)
            && let FileDiff::Rows(rows) = &shown.diff
            && let Some(index) = rows.row_index(lines.side, lines.end, mode)
        {
            shown.draw_around(index);
        }
    }

    fn shown_index(&self) -> Option<usize> {
        let shown = self.shown.as_ref()?;
        self.files.iter().position(|file| file.path == shown.path)
    }

    /// Show the file `delta` places from the shown one, stopping at either
    /// end of what is listed.
    fn show_adjacent(&mut self, delta: isize) {
        let Some(at) = self.shown_index() else {
            return;
        };
        let to = at
            .saturating_add_signed(delta)
            .min(self.files.len().saturating_sub(1));
        if to != at {
            let path = self.files[to].path.clone();
            self.show(&path);
        }
    }

    fn set_mode(&mut self, mode: DiffMode) {
        if self.mode != mode {
            self.mode = mode;
            self.rewindow();
        }
    }

    /// Fold or show the unchanged lines; the shown diff is rebuilt, keeping
    /// how many rows are drawn.
    fn set_unchanged(&mut self, unchanged: Unchanged) {
        if self.unchanged == unchanged {
            return;
        }
        self.unchanged = unchanged;
        if let Some(path) = self.shown.as_ref().map(|shown| shown.path.clone()) {
            let drawn = self.drawn();
            self.build_shown(&path, drawn);
            self.rewindow();
        }
    }

    fn set_revision(&mut self, head: Option<&str>) {
        self.revision = head.map(str::to_owned);
        if let Some(head) = head {
            self.recovery_failures
                .retain(|_, (revision, _)| revision == head);
        }
    }

    /// The pull request's base branch was read as `base`. Moved to another,
    /// what was read whole against the old one is let go, and `true` asks
    /// for the listing, diffed against the old base, to be read again.
    pub(super) fn base_read(&mut self, base: &str) -> bool {
        match self.base.replace(base.to_owned()) {
            Some(old) if old != base => {
                self.recovery_failures.clear();
                self.merge_base = None;
                self.recovering = None;
                !matches!(self.paging, Paging::Reading { page: 1, .. })
            }
            _ => false,
        }
    }

    /// The pull request's head was read as `head`. A listing read at no
    /// known commit is taken to be at it; one read at another commit is
    /// out of date, and `true` asks for it to be read again. A first page
    /// still out was asked for after the head moved, and takes it on landing.
    pub(super) fn head_read(&mut self, head: &str) -> bool {
        match self.revision.as_deref() {
            Some(revision) if revision == head => false,
            Some(_) => !matches!(self.paging, Paging::Reading { page: 1, .. }),
            None => {
                self.set_revision(Some(head));
                if let Some(shown) = self.shown.as_mut()
                    && shown.revision.is_none()
                {
                    shown.revision = Some(head.to_owned());
                }
                self.keep_selection_at(Some(head.to_owned()));
                self.select_pending();
                false
            }
        }
    }

    /// Files from index `first` on just landed. The chosen file's rows are
    /// rebuilt only from a page that lists it, keeping how many are drawn;
    /// a page elsewhere leaves them. With nothing chosen the first file is
    /// shown, so the tab never opens blank; a choice the listing no longer
    /// holds gives way once the `last` page is in.
    fn follow_choice(&mut self, first: usize, last: bool) {
        let Some(chosen) = self.chosen.clone() else {
            if let Some(path) = self.files.first().map(|file| file.path.clone()) {
                self.show(&path);
            }
            return;
        };
        if self.files[first..].iter().any(|file| file.path == chosen) {
            let drawn = self.drawn();
            self.build_shown(&chosen, drawn);
        } else if last && !self.files.iter().any(|file| file.path == chosen) {
            self.chosen = None;
            self.shown = None;
            self.pending_lines = None;
            self.follow_choice(first, last);
        }
    }

    /// The blob of the listed file at `path`, if listed.
    fn blob_of(&self, path: &str) -> Option<Option<&str>> {
        self.files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.blob.as_deref())
    }

    fn show_more_rows(&mut self) {
        if let Some(shown) = self.shown.as_mut() {
            shown.limit += DIFF_ROWS_STEP;
        }
    }

    fn show_earlier_rows(&mut self) {
        if let Some(shown) = self.shown.as_mut() {
            shown.start = shown.start.saturating_sub(DIFF_ROWS_STEP);
        }
    }
}

mod diff;
mod ops;
mod place;
mod recovery;
mod selection;
mod view;

pub(in crate::workspace) use diff::*;
pub(in crate::workspace) use view::*;

#[cfg(test)]
mod tests;
