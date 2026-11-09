//! The scripted answers of one fake repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::base::{AgainstBase, BaseProblem, BaseTips};
use super::super::{
    GitDirs, GitError, GitTracking, GitWorktreeInfo, GitWorktreeStatus, MergeOutcome, RepoProbe,
};
use super::fatal;

/// A repository [`super::FakeGit`] answers for, built field by field. Unset
/// fields answer as [`super::FakeGit`]'s table describes.
#[derive(Debug, Clone)]
pub struct FakeRepo {
    root: PathBuf,
    branch: Option<String>,
    default_branch: Option<String>,
    pub(super) tracking: Option<GitTracking>,
    pub(super) status: Option<GitWorktreeStatus>,
    pub(super) worktrees: Option<Vec<GitWorktreeInfo>>,
    dirs: Option<GitDirs>,
    diffs: HashMap<(PathBuf, bool), String>,
    untracked_diffs: HashMap<PathBuf, String>,
    staged_blobs: HashMap<PathBuf, Vec<u8>>,
    head_message: Option<String>,
    status_scoped: Option<String>,
    base_tips: Option<Result<BaseTips, BaseProblem>>,
    changes_since: Option<Result<AgainstBase, BaseProblem>>,
    diff_range: Option<String>,
    show_at: HashMap<(String, PathBuf), Vec<u8>>,
    merge_outcome: Option<MergeOutcome>,
}

impl FakeRepo {
    /// A repository whose main worktree is `root`.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            branch: None,
            default_branch: None,
            tracking: None,
            status: None,
            worktrees: None,
            dirs: None,
            diffs: HashMap::new(),
            untracked_diffs: HashMap::new(),
            staged_blobs: HashMap::new(),
            head_message: None,
            status_scoped: None,
            base_tips: None,
            changes_since: None,
            diff_range: None,
            show_at: HashMap::new(),
            merge_outcome: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The checked-out branch, which unset `tracking`, `default_branch`, and
    /// `worktrees` also report.
    pub fn branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = Some(branch.into());
        self
    }
    pub fn default_branch(mut self, branch: impl Into<String>) -> Self {
        self.default_branch = Some(branch.into());
        self
    }
    pub fn tracking(mut self, tracking: GitTracking) -> Self {
        self.tracking = Some(tracking);
        self
    }
    pub fn status(mut self, status: GitWorktreeStatus) -> Self {
        self.status = Some(status);
        self
    }
    /// The lanes `probe` lists, main worktree included.
    pub fn worktrees(mut self, worktrees: Vec<GitWorktreeInfo>) -> Self {
        self.worktrees = Some(worktrees);
        self
    }
    pub fn dirs(mut self, dirs: GitDirs) -> Self {
        self.dirs = Some(dirs);
        self
    }
    /// What `diff(path, staged)` prints for exactly that path and side.
    pub fn diff(mut self, path: impl Into<PathBuf>, staged: bool, text: impl Into<String>) -> Self {
        self.diffs.insert((path.into(), staged), text.into());
        self
    }
    pub fn untracked_diff(mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        self.untracked_diffs.insert(path.into(), text.into());
        self
    }
    pub fn staged_blob(mut self, path: impl Into<PathBuf>, bytes: impl Into<Vec<u8>>) -> Self {
        self.staged_blobs.insert(path.into(), bytes.into());
        self
    }
    pub fn head_message(mut self, message: impl Into<String>) -> Self {
        self.head_message = Some(message.into());
        self
    }
    pub fn status_scoped(mut self, text: impl Into<String>) -> Self {
        self.status_scoped = Some(text.into());
        self
    }
    /// The answer to every `base_tips` call that names a base.
    pub fn base_tips(mut self, tips: Result<BaseTips, BaseProblem>) -> Self {
        self.base_tips = Some(tips);
        self
    }
    pub fn changes_since(mut self, changes: Result<AgainstBase, BaseProblem>) -> Self {
        self.changes_since = Some(changes);
        self
    }
    /// What every `diff_range` call prints, whatever range it names.
    pub fn diff_range(mut self, text: impl Into<String>) -> Self {
        self.diff_range = Some(text.into());
        self
    }
    pub fn show_at(
        mut self,
        rev: impl Into<String>,
        path: impl Into<PathBuf>,
        bytes: impl Into<Vec<u8>>,
    ) -> Self {
        self.show_at.insert((rev.into(), path.into()), bytes.into());
        self
    }
    pub fn merge_outcome(mut self, outcome: MergeOutcome) -> Self {
        self.merge_outcome = Some(outcome);
        self
    }

    pub(super) fn probe(&self) -> RepoProbe {
        let lanes = self.worktrees.clone().unwrap_or_else(|| {
            vec![GitWorktreeInfo {
                path: self.root.clone(),
                branch: self.branch.clone(),
                head: None,
                bare: false,
            }]
        });
        RepoProbe {
            repo_root: self.root.clone(),
            lanes,
        }
    }

    pub(super) fn tracking_answer(&self) -> GitTracking {
        self.tracking.clone().unwrap_or_else(|| GitTracking {
            branch: self.branch.clone(),
            ..GitTracking::default()
        })
    }
    pub(super) fn status_answer(&self) -> GitWorktreeStatus {
        self.status.clone().unwrap_or_default()
    }
    pub(super) fn status_scoped_answer(&self) -> String {
        self.status_scoped.clone().unwrap_or_default()
    }
    pub(super) fn diff_answer(&self, path: &Path, staged: bool) -> String {
        self.diffs
            .get(&(path.to_path_buf(), staged))
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn untracked_diff_answer(&self, path: &Path) -> String {
        self.untracked_diffs.get(path).cloned().unwrap_or_default()
    }
    pub(super) fn staged_blob_answer(&self, path: &Path) -> Result<Vec<u8>, GitError> {
        self.staged_blobs.get(path).cloned().ok_or_else(|| {
            fatal(format!(
                "path '{}' exists on disk, but not in the index",
                path.display()
            ))
        })
    }
    pub(super) fn head_message_answer(&self) -> Result<String, GitError> {
        self.head_message
            .clone()
            .ok_or_else(|| fatal("your current branch does not have any commits yet".to_owned()))
    }
    pub(super) fn default_branch_answer(&self) -> Option<String> {
        self.default_branch.clone().or_else(|| self.branch.clone())
    }
    pub(super) fn dirs_answer(&self) -> GitDirs {
        self.dirs.clone().unwrap_or_else(|| GitDirs {
            git_dir: self.root.join(".git"),
            common_dir: self.root.join(".git"),
        })
    }
    pub(super) fn base_tips_answer(&self, base_name: &str) -> Result<BaseTips, BaseProblem> {
        self.base_tips
            .clone()
            .unwrap_or_else(|| Err(BaseProblem::NotFound(base_name.to_owned())))
    }
    pub(super) fn changes_since_answer(&self, tips: BaseTips) -> Result<AgainstBase, BaseProblem> {
        self.changes_since.clone().unwrap_or_else(|| {
            Ok(AgainstBase {
                merge_base: tips.base.sha.clone(),
                tips,
                files: Vec::new(),
                commits: 0,
            })
        })
    }
    pub(super) fn diff_range_answer(&self) -> String {
        self.diff_range.clone().unwrap_or_default()
    }
    pub(super) fn show_at_answer(&self, rev: &str, path: &Path) -> Result<Vec<u8>, GitError> {
        self.show_at
            .get(&(rev.to_owned(), path.to_path_buf()))
            .cloned()
            .ok_or_else(|| {
                fatal(format!(
                    "path '{}' does not exist in '{rev}'",
                    path.display()
                ))
            })
    }
    pub(super) fn merge_answer(&self) -> MergeOutcome {
        self.merge_outcome.clone().unwrap_or(MergeOutcome::Success)
    }
}
