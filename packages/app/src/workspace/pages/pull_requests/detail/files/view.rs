//! What the Files tab shows, built from the state without a window.

use super::*;

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
pub(in super::super) fn files_view(
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

pub(super) fn row(file: &PrFile, shown: Option<&Shown>, viewed: Viewed) -> FileRowView {
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
