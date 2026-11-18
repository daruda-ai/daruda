//! A pull request read in full, with how it may merge.

use super::*;

/// A way GitHub can merge a pull request, as its repository allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// Auto-merge turned on for a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoMergeOn {
    /// `None` for a method this crate does not know.
    pub method: Option<MergeMethod>,
    /// Who turned it on; `None` for a deleted account.
    pub by: Option<String>,
}

/// A pull request in its base branch's merge queue: accepted to merge,
/// not merged yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueEntry {
    /// 1 is next; `None` when GitHub does not say.
    pub position: Option<u32>,
    pub state: QueueState,
}

/// GitHub's `MergeQueueEntryState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueState {
    Queued,
    AwaitingChecks,
    Mergeable,
    Unmergeable,
    Locked,
    /// A state this crate does not know.
    Other,
}

/// Whether a pull request can be merged now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeability {
    /// GitHub would merge it; failing checks that are not required do not
    /// stop a merge, and show in the check list instead.
    Ready,
    Blocked(BlockReason),
    /// The head conflicts with the base.
    Conflicting,
    /// GitHub has not worked it out yet (it does so lazily, after a push),
    /// or answered a value this crate does not know.
    Unknown,
}

/// Why GitHub refuses to merge a pull request that has no conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Branch protection: a required review or check is missing.
    Requirements,
    /// The base requires the head to be up to date with it first.
    Behind,
    /// A draft cannot be merged.
    Draft,
}

/// What GitHub lets the viewer do to a pull request: its `viewerCan*`
/// fields. Read only inside this crate; the app asks `PrDetail`'s
/// decisions (`actions.rs`), which also weigh the pull request's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ViewerCan {
    /// Edit it: its description and reviewers among others.
    pub(crate) update: bool,
    pub(crate) close: bool,
    /// Reopen it once closed.
    pub(crate) reopen: bool,
    pub(crate) enable_auto_merge: bool,
    pub(crate) disable_auto_merge: bool,
}

/// One of [`ViewerCan`]'s permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Update,
    Close,
    Reopen,
    EnableAutoMerge,
    DisableAutoMerge,
}

impl ViewerCan {
    /// Every permission granted.
    pub fn all() -> Self {
        Self {
            update: true,
            close: true,
            reopen: true,
            enable_auto_merge: true,
            disable_auto_merge: true,
        }
    }

    /// `self` with `permission` granted or withheld.
    pub fn with(mut self, permission: Permission, granted: bool) -> Self {
        let slot = match permission {
            Permission::Update => &mut self.update,
            Permission::Close => &mut self.close,
            Permission::Reopen => &mut self.reopen,
            Permission::EnableAutoMerge => &mut self.enable_auto_merge,
            Permission::DisableAutoMerge => &mut self.disable_auto_merge,
        };
        *slot = granted;
        self
    }
}

/// What a pull request's page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDetail {
    pub summary: PrSummary,
    pub body: String,
    /// The head commit, which a merge can be pinned to.
    pub head_sha: String,
    pub base_branch: String,
    /// `None` for a deleted account.
    pub author: Option<String>,
    pub mergeability: Mergeability,
    /// What the repository allows, in the order merge, squash, rebase.
    pub methods: Vec<MergeMethod>,
    /// The method GitHub preselects for the caller: the one they last used
    /// on this repository, else the repository's default. `None` when
    /// GitHub did not say or answered a value this crate does not know.
    pub default_method: Option<MergeMethod>,
    /// Reviewers who reviewed, then those only asked to; a team reads
    /// `org/team`. A request GitHub does not name to the caller is left out.
    pub reviewers: Vec<(String, ReviewerState)>,
    /// Failing, then running, then passing, then skipped; GitHub's order
    /// within each. With more than 100 checks, those of the first page.
    pub checks: Vec<CheckEntry>,
    /// GitHub returned only part of the check list; `summary.checks` still
    /// carries its verdict over the entire list.
    pub checks_truncated: bool,
    /// Unresolved review threads among the first 100.
    pub open_threads: u32,
    /// GitHub has more than 100 threads: `open_threads` counts the first
    /// hundred's only.
    pub open_threads_more: bool,
    /// What GitHub lets the viewer do to it.
    pub viewer_can: ViewerCan,
    /// Set to merge on its own once its requirements pass.
    pub auto_merge: Option<AutoMergeOn>,
    /// Its place in the base branch's merge queue.
    pub queue: Option<QueueEntry>,
    /// GitHub's node id of the pull request, which a review names.
    pub node_id: String,
    /// How many files GitHub says it changes; `None` from an answer that
    /// did not say.
    pub changed_files: Option<u32>,
}
