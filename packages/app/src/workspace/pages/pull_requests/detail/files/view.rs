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
    /// The shortest parent suffix needed to distinguish repeated basenames.
    pub parent: Option<String>,
    pub kind: ChangeKind,
    /// `renamed from old/path`, for a renamed or copied file.
    pub note: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub shown: bool,
    pub viewed: Viewed,
    /// GitHub identifies its content, so it can be marked viewed.
    pub blob: Option<String>,
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

impl ShownView {
    /// Whether diff row `ix` is drawn.
    pub(in crate::workspace) fn draws(&self, ix: usize) -> bool {
        (self.start..self.limit).contains(&ix)
    }
}

/// The chosen file as the tab draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::workspace) struct ShownView {
    pub path: String,
    pub diff: FileDiff,
    /// Rows `start..limit` are drawn; those around them are offered.
    pub start: usize,
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
        picker_open: bool,
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
                picker_open: false,
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
    let parents = state
        .parents
        .get_or_init(|| disambiguating_parents(&state.files));
    let rows: Vec<FileRowView> = state
        .files
        .iter()
        .map(|file| {
            row(
                file,
                state.shown.as_ref(),
                viewed(file),
                parents.get(&file.path).cloned(),
            )
        })
        .collect();
    let shown_row = state
        .shown
        .as_ref()
        .and_then(|shown| rows.iter().find(|row| row.path == shown.path).cloned());
    FilesView::Ready {
        picker_open: state.picker_open,
        viewed: rows.iter().filter(|row| row.viewed == Viewed::Yes).count(),
        files: Rc::new(rows),
        total,
        more,
        shown: state.shown.as_ref().map(|shown| {
            Box::new(ShownView {
                path: shown.path.clone(),
                diff: shown.diff.clone(),
                start: shown.start,
                limit: shown.limit,
                mode: state.mode,
                unchanged: state.unchanged,
                position: state.shown_index().map(|at| (at, state.files.len())),
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

fn disambiguating_parents(files: &[PrFile]) -> HashMap<String, String> {
    let mut counts = HashMap::new();
    for file in files {
        for suffix in suffixes(&file.path) {
            *counts.entry(suffix).or_insert(0usize) += 1;
        }
    }
    files
        .iter()
        .filter_map(|file| {
            let mut tails = suffixes(&file.path).rev();
            let basename = tails.next()?;
            if counts[basename] == 1 {
                return None;
            }
            let distinct = tails.find(|tail| counts[tail] == 1).unwrap_or(&file.path);
            let parent = distinct.rsplit_once('/').map_or(".", |(parent, _)| parent);
            Some((file.path.clone(), parent.to_owned()))
        })
        .collect()
}

fn suffixes(path: &str) -> impl DoubleEndedIterator<Item = &str> {
    std::iter::once(path).chain(path.match_indices('/').map(move |(at, _)| &path[at + 1..]))
}

fn row(
    file: &PrFile,
    shown: Option<&Shown>,
    viewed: Viewed,
    parent: Option<String>,
) -> FileRowView {
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
        parent,
        kind,
        note,
        additions: file.additions,
        deletions: file.deletions,
        shown: shown.is_some_and(|shown| shown.path == file.path),
        viewed,
        blob: file.blob.clone(),
    }
}
