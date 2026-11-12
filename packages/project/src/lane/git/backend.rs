//! The seam in front of the `git` CLI: callers that take [`Git`] can be
//! handed a fake in tests instead of spawning real `git` processes.
//! [`SystemGit`] delegates each method to its `lane::git` free function.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::base::{self, AgainstBase, BaseProblem, BaseTips};
use super::{
    GitDirs, GitError, GitTracking, GitWorktreeStatus, MergeOutcome, PushTarget, RepoProbe,
    discard, remote, status,
};

/// Factory for repository handles plus the operations that precede one.
pub trait Git: Send + Sync {
    /// Whether a usable `git` is installed.
    fn available(&self) -> bool;
    /// `git init` in an existing directory.
    fn init(&self, path: &Path) -> Result<(), GitError>;
    /// The repository and lanes `project_root` belongs to, if any.
    fn probe(&self, project_root: &Path) -> Option<RepoProbe>;
    /// Bind a handle to `worktree`. Spawns nothing and validates nothing, so
    /// it is free to call on a render path; failures surface per command.
    fn open(&self, worktree: &Path) -> Arc<dyn GitRepo>;
}

/// Commands against one worktree, rooted at [`GitRepo::path`].
pub trait GitRepo: Send + Sync {
    fn path(&self) -> &Path;

    fn tracking(&self) -> Result<GitTracking, GitError>;
    fn worktree_status(&self) -> Result<GitWorktreeStatus, GitError>;
    fn status_scoped(&self) -> Result<String, GitError>;
    /// `path` is absolute or relative to [`path`](Self::path); `staged`
    /// selects index-vs-HEAD (`--cached`) instead of worktree-vs-index.
    fn diff(&self, path: &Path, staged: bool) -> Result<String, GitError>;
    /// `path` is absolute or relative to [`path`](Self::path).
    fn diff_untracked(&self, path: &Path) -> Result<String, GitError>;
    /// `path` is relative to the worktree root, whatever the handle path.
    fn show_staged(&self, path: &Path) -> Result<Vec<u8>, GitError>;
    fn head_message(&self) -> Result<String, GitError>;
    fn default_branch(&self) -> Option<String>;
    fn dirs(&self) -> Result<GitDirs, GitError>;
    /// Where `git push` sends the current branch. `None` when HEAD is
    /// detached or no remote can be chosen.
    fn push_target(&self) -> Result<Option<PushTarget>, GitError>;

    /// `path` is absolute or relative to [`path`](Self::path).
    fn add(&self, path: &Path) -> Result<(), GitError>;
    /// Each path is absolute or relative to [`path`](Self::path).
    fn add_paths(&self, paths: &[PathBuf]) -> Result<(), GitError>;
    fn add_all(&self) -> Result<(), GitError>;
    /// As [`restore_staged_paths`](Self::restore_staged_paths) for one path.
    fn restore_staged(&self, path: &Path) -> Result<(), GitError>;
    /// Paths in `git status` form, relative to the worktree root (git reads
    /// them relative to the handle path, so the handle must be rooted at that
    /// worktree root): a rename's source is found by comparing against that
    /// form.
    fn restore_staged_paths(&self, paths: &[PathBuf]) -> Result<(), GitError>;
    fn restore_all_staged(&self) -> Result<(), GitError>;
    /// `pinned` paths are relative to the worktree root, as `git status`
    /// reports them; the handle must be rooted at that worktree root.
    fn discard(&self, pinned: &[PathBuf]) -> Result<(), GitError>;

    fn commit(&self, message: &str) -> Result<(), GitError>;
    fn commit_amend(&self, message: &str) -> Result<(), GitError>;
    fn push(&self) -> Result<(), GitError>;
    /// `git push -u <remote> HEAD:refs/heads/<branch>`, for a branch
    /// [`push_target`](Self::push_target) reports unpublished.
    fn push_set_upstream(&self, remote: &str, branch: &str) -> Result<(), GitError>;
    fn fetch(&self) -> Result<(), GitError>;
    fn pull(&self) -> Result<(), GitError>;

    /// `new_path` must be absolute: the follow-up config step runs git in it,
    /// where a relative path would resolve against the process cwd and be
    /// skipped silently.
    fn add_lane(
        &self,
        new_path: &Path,
        new_branch: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), GitError>;
    /// `path` is the lane's absolute worktree directory: an already-removed
    /// lane is recognized by checking it on the filesystem.
    fn remove_lane(&self, path: &Path, force: bool) -> Result<(), GitError>;
    fn delete_branch(&self, branch: &str) -> Result<(), GitError>;
    fn merge(&self, source_branch: &str) -> Result<MergeOutcome, GitError>;
    fn merge_abort(&self) -> Result<(), GitError>;

    fn base_tips(
        &self,
        base_name: Option<&str>,
        current_branch: Option<&str>,
    ) -> Result<BaseTips, BaseProblem>;
    fn changes_since(&self, tips: BaseTips) -> Result<AgainstBase, BaseProblem>;
    fn diff_range(&self, from: &str, to: &str, paths: &[&Path]) -> Result<String, GitError>;
    /// `repo_rel` is relative to the worktree root, whatever the handle path.
    fn show_at(&self, rev: &str, repo_rel: &Path) -> Result<Vec<u8>, GitError>;
}

/// The installed `git` CLI.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemGit;

impl Git for SystemGit {
    fn available(&self) -> bool {
        super::has_git()
    }
    fn init(&self, path: &Path) -> Result<(), GitError> {
        super::git_init(path)
    }
    fn probe(&self, project_root: &Path) -> Option<RepoProbe> {
        super::probe_repo(project_root)
    }
    fn open(&self, worktree: &Path) -> Arc<dyn GitRepo> {
        Arc::new(SystemRepo {
            path: worktree.to_path_buf(),
        })
    }
}

struct SystemRepo {
    path: PathBuf,
}

impl GitRepo for SystemRepo {
    fn path(&self) -> &Path {
        &self.path
    }

    fn tracking(&self) -> Result<GitTracking, GitError> {
        status::git_tracking(&self.path)
    }
    fn worktree_status(&self) -> Result<GitWorktreeStatus, GitError> {
        status::git_worktree_status(&self.path)
    }
    fn status_scoped(&self) -> Result<String, GitError> {
        status::git_status_scoped(&self.path)
    }
    fn diff(&self, path: &Path, staged: bool) -> Result<String, GitError> {
        status::git_diff(&self.path, path, staged)
    }
    fn diff_untracked(&self, path: &Path) -> Result<String, GitError> {
        status::git_diff_untracked(&self.path, path)
    }
    fn show_staged(&self, path: &Path) -> Result<Vec<u8>, GitError> {
        status::git_show_staged(&self.path, path)
    }
    fn head_message(&self) -> Result<String, GitError> {
        status::git_head_message(&self.path)
    }
    fn default_branch(&self) -> Option<String> {
        super::default_branch(&self.path)
    }
    fn dirs(&self) -> Result<GitDirs, GitError> {
        super::git_dirs(&self.path)
    }
    fn push_target(&self) -> Result<Option<PushTarget>, GitError> {
        remote::push_target(&self.path)
    }

    fn add(&self, path: &Path) -> Result<(), GitError> {
        status::git_add(&self.path, path)
    }
    fn add_paths(&self, paths: &[PathBuf]) -> Result<(), GitError> {
        status::git_add_paths(&self.path, paths)
    }
    fn add_all(&self) -> Result<(), GitError> {
        status::git_add_all(&self.path)
    }
    fn restore_staged(&self, path: &Path) -> Result<(), GitError> {
        status::git_restore_staged(&self.path, path)
    }
    fn restore_staged_paths(&self, paths: &[PathBuf]) -> Result<(), GitError> {
        status::git_restore_staged_paths(&self.path, paths)
    }
    fn restore_all_staged(&self) -> Result<(), GitError> {
        status::git_restore_all_staged(&self.path)
    }
    fn discard(&self, pinned: &[PathBuf]) -> Result<(), GitError> {
        discard::discard(&self.path, pinned)
    }

    fn commit(&self, message: &str) -> Result<(), GitError> {
        status::git_commit(&self.path, message)
    }
    fn commit_amend(&self, message: &str) -> Result<(), GitError> {
        status::git_commit_amend(&self.path, message)
    }
    fn push(&self) -> Result<(), GitError> {
        status::git_push(&self.path)
    }
    fn push_set_upstream(&self, remote: &str, branch: &str) -> Result<(), GitError> {
        remote::push_set_upstream(&self.path, remote, branch)
    }
    fn fetch(&self) -> Result<(), GitError> {
        status::git_fetch(&self.path)
    }
    fn pull(&self) -> Result<(), GitError> {
        status::git_pull(&self.path)
    }

    fn add_lane(
        &self,
        new_path: &Path,
        new_branch: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), GitError> {
        super::add_lane(&self.path, new_path, new_branch, base)
    }
    fn remove_lane(&self, path: &Path, force: bool) -> Result<(), GitError> {
        super::remove_lane(&self.path, path, force)
    }
    fn delete_branch(&self, branch: &str) -> Result<(), GitError> {
        super::delete_branch(&self.path, branch)
    }
    fn merge(&self, source_branch: &str) -> Result<MergeOutcome, GitError> {
        super::git_merge(&self.path, source_branch)
    }
    fn merge_abort(&self) -> Result<(), GitError> {
        super::git_merge_abort(&self.path)
    }

    fn base_tips(
        &self,
        base_name: Option<&str>,
        current_branch: Option<&str>,
    ) -> Result<BaseTips, BaseProblem> {
        base::base_tips(&self.path, base_name, current_branch)
    }
    fn changes_since(&self, tips: BaseTips) -> Result<AgainstBase, BaseProblem> {
        base::changes_since(&self.path, tips)
    }
    fn diff_range(&self, from: &str, to: &str, paths: &[&Path]) -> Result<String, GitError> {
        base::git_diff_range(&self.path, from, to, paths)
    }
    fn show_at(&self, rev: &str, repo_rel: &Path) -> Result<Vec<u8>, GitError> {
        base::git_show_at(&self.path, rev, repo_rel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening a path that is no repository still succeeds: binding is pure,
    /// so a handle can be made before the directory is known to be a repo.
    #[test]
    fn open_binds_the_path_without_checking_it() {
        let missing = Path::new("/nonexistent/daruda/backend-open");
        let repo = SystemGit.open(missing);
        assert_eq!(repo.path(), missing);
    }
}
