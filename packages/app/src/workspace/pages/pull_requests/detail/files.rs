//! The detail's Files tab: the files the pull request changes, read page by
//! page from when the tab is first shown, and the unified diff of the one
//! chosen. Reading stops at a transfer budget and resumes on request; a
//! page that fails keeps what came before it. The diff's rows are built
//! once per choice and drawn a bounded number at a time.

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

/// A file's diff rows, unified and paired side by side, shared by the
/// state and each frame's view. Equal only to itself: comparing two diffs
/// row by row is not worth a frame.
#[derive(Clone)]
pub(in crate::workspace) struct DiffRows {
    unified: Arc<Vec<VisualRow>>,
    split: Arc<Vec<SplitRow>>,
}

impl DiffRows {
    fn new(rows: Vec<VisualRow>) -> Self {
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
    fn of(file: &PrFile, unchanged: Unchanged) -> Self {
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

/// Patch text read before pages stop being read on their own: a pull
/// request this large is read further only when asked.
const AUTO_READ_BYTES: usize = 4 * 1024 * 1024;
/// Diff rows drawn at first, and added by each "show more".
pub(in crate::workspace) const DIFF_ROWS_STEP: usize = 1_000;

/// The chosen file, by path, its diff, and how many of its rows are drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    path: String,
    diff: FileDiff,
    limit: usize,
    /// The head commit its rows were read at, once known: what lines
    /// selected in them are pinned to. An earlier listing's rows stay up
    /// through a refresh under their own commit.
    revision: Option<String>,
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
    /// The merge base a file read whole was diffed from, with the head it
    /// was found for: the same for every file of one listing.
    merge_base: Option<(String, String)>,
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
    failure: Option<RecoverFailure>,
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
            replacing: false,
            paging: Paging::Reading { ticket, page: 1 },
            capped: false,
            patch_bytes: 0,
            budget: AUTO_READ_BYTES,
            chosen: None,
            shown: None,
            mode: DiffMode::default(),
            unchanged: Unchanged::default(),
            revision: None,
            selection: None,
            restore: None,
            recovering: None,
            merge_base: None,
        }
    }

    /// Read again from the first page; the listing and the chosen file stay
    /// up until it lands.
    fn reread(&mut self, ticket: ReadTicket) {
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
            self.revision = head.map(str::to_owned);
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
        self.chosen = Some(path.to_owned());
        if self.shown.as_ref().is_some_and(|shown| shown.path == path) {
            return;
        }
        self.build_shown(path, DIFF_ROWS_STEP);
    }

    fn build_shown(&mut self, path: &str, limit: usize) {
        if let Some(file) = self.files.iter().find(|file| file.path == path) {
            self.shown = Some(Shown {
                path: file.path.clone(),
                diff: FileDiff::of(file, self.unchanged),
                limit,
                revision: self.revision.clone(),
            });
            self.keep_selection_at(self.revision.clone());
        }
    }

    /// Lines selected at another commit than `revision` no longer point at
    /// the code shown.
    fn keep_selection_at(&mut self, revision: Option<String>) {
        if let Some(revision) = revision
            && self
                .selection
                .as_ref()
                .is_some_and(|held| held.revision != revision)
        {
            self.selection = None;
        }
    }

    /// Show the file `delta` places from the shown one, stopping at either
    /// end of what is listed.
    fn show_adjacent(&mut self, delta: isize) {
        let Some(at) = self
            .shown
            .as_ref()
            .and_then(|shown| self.files.iter().position(|file| file.path == shown.path))
        else {
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
        self.mode = mode;
    }

    /// Fold or show the unchanged lines; the shown diff is rebuilt, keeping
    /// how many rows are drawn.
    fn set_unchanged(&mut self, unchanged: Unchanged) {
        if self.unchanged == unchanged {
            return;
        }
        self.unchanged = unchanged;
        if let Some(shown) = self.shown.as_ref() {
            let (path, limit) = (shown.path.clone(), shown.limit);
            self.build_shown(&path, limit);
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
                self.revision = Some(head.to_owned());
                if let Some(shown) = self.shown.as_mut()
                    && shown.revision.is_none()
                {
                    shown.revision = Some(head.to_owned());
                }
                self.keep_selection_at(Some(head.to_owned()));
                false
            }
        }
    }

    /// Select `line` on `side` of the shown file, or stretch the selection
    /// to it. Nothing while the commit its rows were read at is unknown.
    fn select_line(&mut self, side: Side, line: u32, extend: bool) {
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

    fn clear_selection(&mut self) {
        self.selection = None;
    }

    #[cfg(test)]
    pub(in crate::workspace) fn selection(&self) -> Option<&LineRange> {
        self.selection.as_ref()
    }

    /// The selection's reference, and below it the selected lines when the
    /// shown file is the one they are in.
    fn selection_copy(&self) -> Option<String> {
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
            let limit = self
                .shown
                .as_ref()
                .map_or(DIFF_ROWS_STEP, |shown| shown.limit);
            self.build_shown(&chosen, limit);
        } else if last && !self.files.iter().any(|file| file.path == chosen) {
            self.chosen = None;
            self.shown = None;
            self.follow_choice(first, last);
        }
    }

    /// The shown file and its content, to come back to.
    pub(super) fn place_file(&self) -> Option<PlaceFile> {
        let path = &self.shown.as_ref()?.path;
        let file = self.files.iter().find(|file| &file.path == path)?;
        Some(PlaceFile {
            path: path.clone(),
            blob: file.blob.clone(),
        })
    }

    /// Come back to `file`, scrolled to `offset` if its content is the
    /// same: chosen now, shown once a page lists it.
    pub(super) fn restore_to(&mut self, file: &PlaceFile, offset: (f32, f32)) {
        self.chosen = Some(file.path.clone());
        self.restore = Some(Restore {
            path: file.path.clone(),
            blob: file.blob.clone(),
            offset,
        });
    }

    /// Once the file come back to is shown, where it is and the offset to
    /// scroll to; given once. Dropped when reading ends without it.
    pub(super) fn take_restored(&mut self) -> Option<Restored> {
        let path = &self.restore.as_ref()?.path;
        let at = self
            .shown
            .as_ref()
            .filter(|shown| &shown.path == path)
            .and_then(|shown| self.files.iter().position(|file| file.path == shown.path));
        let Some(index) = at else {
            if !matches!(self.paging, Paging::Reading { .. }) {
                self.restore = None;
            }
            return None;
        };
        let restore = self.restore.take()?;
        let same = restore.blob.is_some() && self.files[index].blob == restore.blob;
        Some(Restored {
            index,
            offset: same.then_some(restore.offset),
        })
    }

    #[cfg(test)]
    pub(in crate::workspace) fn coming_back(&self) -> bool {
        self.restore.is_some()
    }

    /// The place a reopened detail is still coming back to.
    pub(super) fn pending_place(&self) -> Option<(PlaceFile, (f32, f32))> {
        let restore = self.restore.as_ref()?;
        Some((
            PlaceFile {
                path: restore.path.clone(),
                blob: restore.blob.clone(),
            },
            restore.offset,
        ))
    }

    /// Start reading the shown file whole: what to read, at the listing's
    /// commit. `None` unless GitHub sent no patch for it and the commit is
    /// known.
    fn begin_recover(&mut self, ticket: ReadTicket) -> Option<RecoverStart> {
        if self.recovery_busy() {
            return None;
        }
        let shown = self
            .shown
            .as_ref()
            .filter(|shown| shown.diff == FileDiff::TooLarge)?;
        let revision = self.revision.clone()?;
        let file = self.files.iter().find(|file| file.path == shown.path)?;
        self.recovering = Some(Recovering {
            path: file.path.clone(),
            revision: revision.clone(),
            ticket,
            failure: None,
        });
        Some(RecoverStart {
            path: file.path.clone(),
            change: file.change.clone(),
            lines: file.additions + file.deletions,
            merge_base: self
                .merge_base
                .as_ref()
                .filter(|(head, _)| *head == revision)
                .map(|(_, base)| base.clone()),
            head: revision,
        })
    }

    /// A file is being read whole.
    fn recovery_busy(&self) -> bool {
        self.recovering
            .as_ref()
            .is_some_and(|held| held.failure.is_none())
    }

    /// Take the whole file's diff if it is the read waited on and the
    /// listing is still at its commit: it becomes the file's patch, as if
    /// GitHub had sent it.
    fn land_recovered(
        &mut self,
        ticket: ReadTicket,
        answer: Result<Recovered, RecoverFailure>,
    ) -> bool {
        let Some(recovering) = self
            .recovering
            .as_mut()
            .filter(|held| held.ticket == ticket && self.revision.as_ref() == Some(&held.revision))
        else {
            return false;
        };
        let read = match answer {
            Ok(read) => read,
            Err(failure) => {
                recovering.failure = Some(failure);
                return true;
            }
        };
        let path = recovering.path.clone();
        self.merge_base = Some((recovering.revision.clone(), read.merge_base));
        self.recovering = None;
        if let Some(file) = self.files.iter_mut().find(|file| file.path == path) {
            file.patch = Some(read.patch);
        }
        if let Some(limit) = self
            .shown
            .as_ref()
            .filter(|shown| shown.path == path)
            .map(|shown| shown.limit)
        {
            self.build_shown(&path, limit);
        }
        true
    }

    fn recovery_view(&self, shown: &Shown) -> Option<RecoveryView> {
        if shown.diff != FileDiff::TooLarge || self.revision.is_none() {
            return None;
        }
        Some(
            match self
                .recovering
                .as_ref()
                .filter(|held| held.path == shown.path)
            {
                None if self.recovery_busy() => RecoveryView::Busy,
                None => RecoveryView::Offered,
                Some(Recovering {
                    failure: Some(failure),
                    ..
                }) => RecoveryView::Failed(failure.clone()),
                Some(_) => RecoveryView::Reading,
            },
        )
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
}

/// How a listed file changed, as its row shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::workspace) enum ChangeKind {
    Added,
    Removed,
    Modified,
    Renamed,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct FileRowView {
    pub path: String,
    pub kind: ChangeKind,
    /// `renamed from old/path`, for a renamed or copied file.
    pub note: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub shown: bool,
    pub viewed: Viewed,
    /// GitHub identifies its content, so it can be marked viewed.
    pub markable: bool,
}

/// How far the listing is read, below the files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum FilesMore {
    /// Every file is listed.
    None,
    /// More pages are being read.
    Reading,
    /// The listing shown is being read again from the top.
    Refreshing,
    /// Reading it again from the top failed; the earlier listing stays.
    RefreshFailed(String),
    /// Reading stopped at the budget: read the rest on request.
    Paused,
    /// A page failed: try it again on request.
    Failed(String),
    /// GitHub lists no more than these.
    Capped,
}

/// The chosen file as the tab draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ShownView {
    pub path: String,
    pub diff: FileDiff,
    /// Rows drawn; the rest are offered.
    pub limit: usize,
    pub mode: DiffMode,
    pub unchanged: Unchanged,
    /// Its place in the listing and the listing's length, for the
    /// previous and next file; `None` while a refresh has not listed it yet.
    pub position: Option<(usize, usize)>,
    /// The lines selected in it.
    pub selection: Option<LineRange>,
    /// Whether its lines can be selected: the commit its rows were read at
    /// is known.
    pub selectable: bool,
    /// Its row in the listing, for its viewed mark; `None` while unlisted.
    pub row: Option<FileRowView>,
    /// When GitHub sent no patch: reading it whole.
    pub recovery: Option<RecoveryView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) enum FilesView {
    Loading,
    Failed(String),
    Ready {
        /// Shared with the virtualized list, which outlives the frame.
        files: Rc<Vec<FileRowView>>,
        /// How many files GitHub says change, when it said.
        total: Option<u32>,
        /// How many of those listed are marked viewed at their content.
        viewed: usize,
        more: FilesMore,
        /// Boxed: a diff's view outweighs the other states many times.
        shown: Option<Box<ShownView>>,
    },
}

/// The tab for `files`; `total` is GitHub's count of changed files.
pub(super) fn files_view(
    files: Option<&FilesState>,
    total: Option<u32>,
    viewed: impl Fn(&PrFile) -> Viewed,
) -> FilesView {
    let Some(state) = files else {
        return FilesView::Loading;
    };
    if state.files.is_empty() {
        return match &state.paging {
            Paging::Failed { reason, .. } => FilesView::Failed(reason.clone()),
            Paging::Reading { .. } => FilesView::Loading,
            Paging::Paused { .. } | Paging::Done => FilesView::Ready {
                viewed: 0,
                files: Rc::default(),
                total,
                more: FilesMore::None,
                shown: None,
            },
        };
    }
    let more = match &state.paging {
        Paging::Reading { .. } if state.replacing => FilesMore::Refreshing,
        Paging::Failed { reason, .. } if state.replacing => {
            FilesMore::RefreshFailed(reason.clone())
        }
        Paging::Reading { .. } => FilesMore::Reading,
        Paging::Paused { .. } => FilesMore::Paused,
        Paging::Failed { reason, .. } => FilesMore::Failed(reason.clone()),
        Paging::Done if state.capped => FilesMore::Capped,
        Paging::Done => FilesMore::None,
    };
    // Once every page is in, the listing is the count; GitHub's number,
    // read separately, could disagree with it.
    let total = total.filter(|_| !state.complete());
    let rows: Vec<FileRowView> = state
        .files
        .iter()
        .map(|file| row(file, state.shown.as_ref(), viewed(file)))
        .collect();
    let shown_row = state
        .shown
        .as_ref()
        .and_then(|shown| rows.iter().find(|row| row.path == shown.path).cloned());
    FilesView::Ready {
        viewed: rows.iter().filter(|row| row.viewed == Viewed::Yes).count(),
        files: Rc::new(rows),
        total,
        more,
        shown: state.shown.as_ref().map(|shown| {
            Box::new(ShownView {
                path: shown.path.clone(),
                diff: shown.diff.clone(),
                limit: shown.limit,
                mode: state.mode,
                unchanged: state.unchanged,
                position: state
                    .files
                    .iter()
                    .position(|file| file.path == shown.path)
                    .map(|at| (at, state.files.len())),
                selection: state
                    .selection
                    .clone()
                    .filter(|selection| selection.path == shown.path),
                selectable: shown.revision.is_some(),
                row: shown_row,
                recovery: state.recovery_view(shown),
            })
        }),
    }
}

fn row(file: &PrFile, shown: Option<&Shown>, viewed: Viewed) -> FileRowView {
    let (kind, note) = match &file.change {
        FileChange::Added => (ChangeKind::Added, None),
        FileChange::Removed => (ChangeKind::Removed, None),
        FileChange::Modified => (ChangeKind::Modified, None),
        FileChange::Renamed { from } => (ChangeKind::Renamed, Some(s::file_renamed_from(from))),
        FileChange::Copied { from } => (ChangeKind::Other, Some(s::file_copied_from(from))),
        FileChange::Changed | FileChange::Unchanged => (ChangeKind::Other, None),
    };
    FileRowView {
        path: file.path.clone(),
        kind,
        note,
        additions: file.additions,
        deletions: file.deletions,
        shown: shown.is_some_and(|shown| shown.path == file.path),
        viewed,
        markable: file.blob.is_some(),
    }
}

impl PrDetailState {
    /// Bring a reopened detail back to the file it showed, once that file
    /// is shown and the pull request is read, so the tab is drawn at its
    /// full height to scroll in.
    pub(super) fn come_back(&mut self) {
        if !matches!(self.pr.load(), super::Load::Ready(_)) || self.tab != super::PrTab::Files {
            return;
        }
        if let Some(restored) = self.files.as_mut().and_then(FilesState::take_restored) {
            self.files_scroll
                .scroll_to_item(restored.index, gpui::ScrollStrategy::Center);
            if let Some((x, y)) = restored.offset {
                self.scroll
                    .set_offset(gpui::point(gpui::px(x), gpui::px(y)));
            }
        }
    }

    /// The person moved on: a reopened detail no longer comes back.
    pub(super) fn stop_coming_back(&mut self) {
        if let Some(files) = self.files.as_mut() {
            files.restore = None;
        }
    }

    pub(super) fn begin_files_read(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        self.files = Some(FilesState::reading(ticket));
        ticket
    }

    /// Read the shown files again in place.
    pub(super) fn reread_files(&mut self) -> ReadTicket {
        let ticket = self.tickets.issue();
        match self.files.as_mut() {
            Some(files) => files.reread(ticket),
            None => self.files = Some(FilesState::reading(ticket)),
        }
        ticket
    }

    pub(in crate::workspace) fn files(&self) -> Option<&FilesState> {
        self.files.as_ref()
    }
}

impl Workspace {
    pub(super) fn send_pr_files_read(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        page: u32,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let (repo, number) = (detail.item.repo.clone(), detail.item.pr.number);
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || gh.pr_files_page(&repo, number, page),
            move |ws, result, cx| ws.land_pr_files(id, ticket, page, result, cx),
        )
        .detach();
    }

    /// Only the page waited on lands; a further page is sent at once.
    pub(in crate::workspace) fn land_pr_files(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        page: u32,
        result: Result<PrFilesPage, GhError>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let PrDetailState { files, tickets, .. } = detail;
        let Some(state) = files.as_mut() else {
            return;
        };
        let (answer, error) = match result {
            Ok(read) => (Ok(read), None),
            Err(e) => (Err(e.to_string()), Some(e)),
        };
        // The head as last read, unless a read of it is out: then the files
        // take it when that read lands.
        let head = detail.pr.current().map(|pr| pr.head_sha.as_str());
        let landed = state.land(ticket, page, answer, head, || tickets.issue());
        if landed == Landed::Stale {
            return;
        }
        detail.come_back();
        if let Some(e) = error {
            self.report_pr_read(&e, "github.pr_files.read", cx);
        }
        if let Landed::Next(ticket, page) = landed {
            self.send_pr_files_read(id, ticket, page, cx);
        }
        cx.notify();
    }

    /// Read the files on from where reading stopped or failed.
    pub(in crate::workspace) fn resume_pr_files(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let ticket = detail.tickets.issue();
        let Some(page) = detail.files.as_mut().and_then(|files| files.resume(ticket)) else {
            return;
        };
        self.send_pr_files_read(id, ticket, page, cx);
        cx.notify();
    }

    /// Show `path`'s diff in detail `id`'s Files tab.
    pub(in crate::workspace) fn show_pr_file(
        &mut self,
        id: PrDetailId,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        detail.stop_coming_back();
        if let Some(state) = detail.files.as_mut() {
            state.show(path);
            cx.notify();
        }
    }

    fn with_pr_files(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut FilesState),
    ) {
        if let Some(state) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) {
            change(state);
            cx.notify();
        }
    }

    /// Mark `path` viewed at its listed content, or clear the mark.
    pub(in crate::workspace) fn toggle_pr_file_viewed(
        &mut self,
        id: PrDetailId,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let key = detail.key();
        let Some(blob) = detail
            .files
            .as_ref()
            .and_then(|files| files.blob_of(path))
            .map(|blob| blob.map(str::to_owned))
        else {
            return;
        };
        self.pages
            .pull_requests
            .progress
            .toggle_viewed(&key, path, blob.as_deref());
        cx.notify();
    }

    /// Show the file `delta` places from the shown one.
    pub(in crate::workspace) fn show_adjacent_pr_file(
        &mut self,
        id: PrDetailId,
        delta: isize,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| {
            state.restore = None;
            state.show_adjacent(delta);
        });
        // Keep the shown file's row in view in a long listing.
        let at = self.pr_detail_mut(id).and_then(|detail| {
            let state = detail.files.as_ref()?;
            let path = &state.shown.as_ref()?.path;
            let at = state.files.iter().position(|file| &file.path == path)?;
            Some((detail.files_scroll.clone(), at))
        });
        if let Some((scroll, at)) = at {
            scroll.scroll_to_item(at, gpui::ScrollStrategy::Nearest);
        }
    }

    pub(in crate::workspace) fn set_pr_diff_mode(
        &mut self,
        id: PrDetailId,
        mode: DiffMode,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.set_mode(mode));
    }

    pub(in crate::workspace) fn set_pr_diff_unchanged(
        &mut self,
        id: PrDetailId,
        unchanged: Unchanged,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.set_unchanged(unchanged));
    }

    /// Select a line of the shown file; `extend` stretches the selection.
    pub(in crate::workspace) fn select_pr_line(
        &mut self,
        id: PrDetailId,
        side: Side,
        line: u32,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        self.with_pr_files(id, cx, |state| state.select_line(side, line, extend));
    }

    pub(in crate::workspace) fn clear_pr_lines(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        self.with_pr_files(id, cx, FilesState::clear_selection);
    }

    /// Copy the selected lines' reference: path, lines, side, revision.
    pub(in crate::workspace) fn copy_pr_lines(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let copied = self
            .pr_detail_mut(id)
            .and_then(|d| d.files.as_ref())
            .and_then(FilesState::selection_copy);
        if let Some(copied) = copied {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(copied));
        }
    }

    /// Read the shown file's diff whole: GitHub sent no patch for it.
    pub(in crate::workspace) fn recover_pr_file(&mut self, id: PrDetailId, cx: &mut Context<Self>) {
        let Some(detail) = self.pr_detail_mut(id) else {
            return;
        };
        let Some(base_branch) = detail.pr.current().map(|pr| pr.base_branch.clone()) else {
            return;
        };
        let ticket = detail.tickets.issue();
        let Some(start) = detail
            .files
            .as_mut()
            .and_then(|files| files.begin_recover(ticket))
        else {
            return;
        };
        let target = RecoverTarget {
            repo: detail.item.repo.clone(),
            base_branch,
            head: start.head,
            merge_base: start.merge_base,
            path: start.path,
            change: start.change,
            lines: start.lines,
        };
        let gh = GhHost::global(cx);
        spawn_bg_work_and_mutate(
            cx,
            move || recover_patch(gh.as_ref(), &target),
            move |ws, result, cx| ws.land_pr_recovered(id, ticket, result, cx),
        )
        .detach();
        cx.notify();
    }

    /// Only the read waited on lands; a `gh` failure is reported too.
    pub(in crate::workspace) fn land_pr_recovered(
        &mut self,
        id: PrDetailId,
        ticket: ReadTicket,
        result: Result<Recovered, RecoverError>,
        cx: &mut Context<Self>,
    ) {
        let Some(files) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) else {
            return;
        };
        let (answer, cause) = match result {
            Ok(read) => (Ok(read), None),
            Err(error) => (Err(error.failure), error.cause),
        };
        if !files.land_recovered(ticket, answer) {
            return;
        }
        if let Some(e) = cause {
            self.report_pr_read(&e, "github.pr_file.recover", cx);
        }
        cx.notify();
    }

    /// Draw more of the shown diff's rows.
    pub(in crate::workspace) fn show_more_pr_diff(
        &mut self,
        id: PrDetailId,
        cx: &mut Context<Self>,
    ) {
        if let Some(state) = self.pr_detail_mut(id).and_then(|d| d.files.as_mut()) {
            state.show_more_rows();
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests;
