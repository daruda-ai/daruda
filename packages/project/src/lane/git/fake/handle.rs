//! The [`GitRepo`] a [`super::FakeGit`] hands out.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::super::base::{AgainstBase, BaseProblem, BaseTips};
use super::super::{GitDirs, GitError, GitRepo, GitTracking, GitWorktreeStatus, MergeOutcome};
use super::{FakeRepo, FakeState, GitCall, GitOp, lock, not_a_repo};

pub(super) struct FakeHandle {
    path: PathBuf,
    state: Arc<Mutex<FakeState>>,
}

impl FakeHandle {
    pub(super) fn new(path: PathBuf, state: Arc<Mutex<FakeState>>) -> Self {
        Self { path, state }
    }

    /// Count `op`, pass the gate, then answer from the enclosing repository.
    fn read<T>(
        &self,
        op: GitOp,
        answer: impl FnOnce(&FakeRepo) -> Result<T, GitError>,
    ) -> Result<T, GitError> {
        let mut state = lock(&self.state);
        state.count(op, &self.path);
        state.gate(op, &self.path)?;
        state
            .repo_for(&self.path)
            .ok_or_else(not_a_repo)
            .and_then(answer)
    }

    /// Like [`Self::read`], but the call is logged before anything can fail:
    /// the log records what was asked for, not what took effect.
    fn write<T>(
        &self,
        op: GitOp,
        call: GitCall,
        answer: impl FnOnce(&FakeRepo) -> T,
    ) -> Result<T, GitError> {
        let mut state = lock(&self.state);
        state.calls.push(call);
        drop(state);
        self.read(op, |repo| Ok(answer(repo)))
    }

    fn base_read<T>(
        &self,
        op: GitOp,
        answer: impl FnOnce(&FakeRepo) -> Result<T, BaseProblem>,
    ) -> Result<T, BaseProblem> {
        let mut state = lock(&self.state);
        state.count(op, &self.path);
        state.gate(op, &self.path)?;
        let repo = state.repo_for(&self.path).ok_or_else(not_a_repo)?;
        answer(repo)
    }

    fn repo(&self) -> PathBuf {
        self.path.clone()
    }
}

impl GitRepo for FakeHandle {
    fn path(&self) -> &Path {
        &self.path
    }

    fn tracking(&self) -> Result<GitTracking, GitError> {
        self.read(GitOp::Tracking, |repo| Ok(repo.tracking_answer()))
    }
    fn worktree_status(&self) -> Result<GitWorktreeStatus, GitError> {
        self.read(GitOp::WorktreeStatus, |repo| Ok(repo.status_answer()))
    }
    fn status_scoped(&self) -> Result<String, GitError> {
        self.read(GitOp::StatusScoped, |repo| Ok(repo.status_scoped_answer()))
    }
    fn diff(&self, path: &Path, staged: bool) -> Result<String, GitError> {
        self.read(GitOp::Diff, |repo| Ok(repo.diff_answer(path, staged)))
    }
    fn diff_untracked(&self, path: &Path) -> Result<String, GitError> {
        self.read(GitOp::DiffUntracked, |repo| {
            Ok(repo.untracked_diff_answer(path))
        })
    }
    fn show_staged(&self, path: &Path) -> Result<Vec<u8>, GitError> {
        self.read(GitOp::ShowStaged, |repo| repo.staged_blob_answer(path))
    }
    fn head_message(&self) -> Result<String, GitError> {
        self.read(GitOp::HeadMessage, FakeRepo::head_message_answer)
    }
    fn default_branch(&self) -> Option<String> {
        self.read(
            GitOp::DefaultBranch,
            |repo| Ok(repo.default_branch_answer()),
        )
        .ok()
        .flatten()
    }
    fn dirs(&self) -> Result<GitDirs, GitError> {
        self.read(GitOp::Dirs, |repo| Ok(repo.dirs_answer()))
    }

    fn add(&self, path: &Path) -> Result<(), GitError> {
        let call = GitCall::Add {
            repo: self.repo(),
            path: path.to_path_buf(),
        };
        self.write(GitOp::Add, call, |_| ())
    }
    fn add_paths(&self, paths: &[PathBuf]) -> Result<(), GitError> {
        let call = GitCall::AddPaths {
            repo: self.repo(),
            paths: paths.to_vec(),
        };
        self.write(GitOp::AddPaths, call, |_| ())
    }
    fn add_all(&self) -> Result<(), GitError> {
        let call = GitCall::AddAll { repo: self.repo() };
        self.write(GitOp::AddAll, call, |_| ())
    }
    fn restore_staged(&self, path: &Path) -> Result<(), GitError> {
        let call = GitCall::RestoreStaged {
            repo: self.repo(),
            path: path.to_path_buf(),
        };
        self.write(GitOp::RestoreStaged, call, |_| ())
    }
    fn restore_staged_paths(&self, paths: &[PathBuf]) -> Result<(), GitError> {
        let call = GitCall::RestoreStagedPaths {
            repo: self.repo(),
            paths: paths.to_vec(),
        };
        self.write(GitOp::RestoreStagedPaths, call, |_| ())
    }
    fn restore_all_staged(&self) -> Result<(), GitError> {
        let call = GitCall::RestoreAllStaged { repo: self.repo() };
        self.write(GitOp::RestoreAllStaged, call, |_| ())
    }
    fn discard(&self, pinned: &[PathBuf]) -> Result<(), GitError> {
        let call = GitCall::Discard {
            repo: self.repo(),
            pinned: pinned.to_vec(),
        };
        self.write(GitOp::Discard, call, |_| ())
    }

    fn commit(&self, message: &str) -> Result<(), GitError> {
        let call = GitCall::Commit {
            repo: self.repo(),
            message: message.to_owned(),
        };
        self.write(GitOp::Commit, call, |_| ())
    }
    fn commit_amend(&self, message: &str) -> Result<(), GitError> {
        let call = GitCall::CommitAmend {
            repo: self.repo(),
            message: message.to_owned(),
        };
        self.write(GitOp::CommitAmend, call, |_| ())
    }
    fn push(&self) -> Result<(), GitError> {
        let call = GitCall::Push { repo: self.repo() };
        self.write(GitOp::Push, call, |_| ())
    }
    fn fetch(&self) -> Result<(), GitError> {
        let call = GitCall::Fetch { repo: self.repo() };
        self.write(GitOp::Fetch, call, |_| ())
    }
    fn pull(&self) -> Result<(), GitError> {
        let call = GitCall::Pull { repo: self.repo() };
        self.write(GitOp::Pull, call, |_| ())
    }

    fn add_lane(
        &self,
        new_path: &Path,
        new_branch: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), GitError> {
        let call = GitCall::AddLane {
            repo: self.repo(),
            new_path: new_path.to_path_buf(),
            new_branch: new_branch.map(str::to_owned),
            base: base.map(str::to_owned),
        };
        self.write(GitOp::AddLane, call, |_| ())
    }
    fn remove_lane(&self, path: &Path, force: bool) -> Result<(), GitError> {
        let call = GitCall::RemoveLane {
            repo: self.repo(),
            path: path.to_path_buf(),
            force,
        };
        self.write(GitOp::RemoveLane, call, |_| ())
    }
    fn delete_branch(&self, branch: &str) -> Result<(), GitError> {
        let call = GitCall::DeleteBranch {
            repo: self.repo(),
            branch: branch.to_owned(),
        };
        self.write(GitOp::DeleteBranch, call, |_| ())
    }
    fn merge(&self, source_branch: &str) -> Result<MergeOutcome, GitError> {
        let call = GitCall::Merge {
            repo: self.repo(),
            source_branch: source_branch.to_owned(),
        };
        self.write(GitOp::Merge, call, FakeRepo::merge_answer)
    }
    fn merge_abort(&self) -> Result<(), GitError> {
        let call = GitCall::MergeAbort { repo: self.repo() };
        self.write(GitOp::MergeAbort, call, |_| ())
    }

    fn base_tips(
        &self,
        base_name: Option<&str>,
        current_branch: Option<&str>,
    ) -> Result<BaseTips, BaseProblem> {
        // The real check precedes any git process, so it holds even
        // without a repository or a git install.
        let Some(name) = base_name else {
            lock(&self.state).count(GitOp::BaseTips, &self.path);
            return Err(BaseProblem::NoBaseConfigured);
        };
        self.base_read(GitOp::BaseTips, |repo| {
            let tips = repo.base_tips_answer(name)?;
            // As in the real one: a resolved base the lane is checked out on.
            if current_branch.is_some_and(|b| b == name || b == tips.base.label) {
                return Err(BaseProblem::OnBaseBranch);
            }
            Ok(tips)
        })
    }
    fn changes_since(&self, tips: BaseTips) -> Result<AgainstBase, BaseProblem> {
        self.base_read(GitOp::ChangesSince, |repo| repo.changes_since_answer(tips))
    }
    fn diff_range(&self, _from: &str, _to: &str, _paths: &[&Path]) -> Result<String, GitError> {
        self.read(GitOp::DiffRange, |repo| Ok(repo.diff_range_answer()))
    }
    fn show_at(&self, rev: &str, repo_rel: &Path) -> Result<Vec<u8>, GitError> {
        self.read(GitOp::ShowAt, |repo| repo.show_at_answer(rev, repo_rel))
    }
}
