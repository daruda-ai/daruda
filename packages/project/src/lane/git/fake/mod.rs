//! A scripted [`Git`] for tests: answers come from registered [`FakeRepo`]s,
//! writes are logged as [`GitCall`]s and never applied, and no `git` process
//! is spawned. [`FakeGit`] lists what it answers by default.

mod handle;
mod repo;

pub use repo::FakeRepo;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{Git, GitError, GitRepo, GitTracking, GitWorktreeInfo, GitWorktreeStatus, RepoProbe};

/// A write the code under test asked for. `repo` is the path of the handle
/// the call was made on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitCall {
    Init {
        path: PathBuf,
    },
    Add {
        repo: PathBuf,
        path: PathBuf,
    },
    AddPaths {
        repo: PathBuf,
        paths: Vec<PathBuf>,
    },
    AddAll {
        repo: PathBuf,
    },
    RestoreStaged {
        repo: PathBuf,
        path: PathBuf,
    },
    RestoreStagedPaths {
        repo: PathBuf,
        paths: Vec<PathBuf>,
    },
    RestoreAllStaged {
        repo: PathBuf,
    },
    Discard {
        repo: PathBuf,
        pinned: Vec<PathBuf>,
    },
    Commit {
        repo: PathBuf,
        message: String,
    },
    CommitAmend {
        repo: PathBuf,
        message: String,
    },
    Push {
        repo: PathBuf,
    },
    PushSetUpstream {
        repo: PathBuf,
        remote: String,
        branch: String,
    },
    Fetch {
        repo: PathBuf,
    },
    Pull {
        repo: PathBuf,
    },
    AddLane {
        repo: PathBuf,
        new_path: PathBuf,
        new_branch: Option<String>,
        base: Option<String>,
    },
    RemoveLane {
        repo: PathBuf,
        path: PathBuf,
        force: bool,
    },
    DeleteBranch {
        repo: PathBuf,
        branch: String,
    },
    Merge {
        repo: PathBuf,
        source_branch: String,
    },
    MergeAbort {
        repo: PathBuf,
    },
}

/// One [`Git`] or [`GitRepo`] method, for counting calls and aiming failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GitOp {
    Available,
    Init,
    Probe,
    Open,
    Tracking,
    WorktreeStatus,
    StatusScoped,
    Diff,
    DiffUntracked,
    ShowStaged,
    HeadMessage,
    DefaultBranch,
    Dirs,
    PushTarget,
    Add,
    AddPaths,
    AddAll,
    RestoreStaged,
    RestoreStagedPaths,
    RestoreAllStaged,
    Discard,
    Commit,
    CommitAmend,
    Push,
    PushSetUpstream,
    Fetch,
    Pull,
    AddLane,
    RemoveLane,
    DeleteBranch,
    Merge,
    MergeAbort,
    BaseTips,
    ChangesSince,
    DiffRange,
    ShowAt,
}

/// A cloneable description of a [`GitError`], turned into a fresh error on
/// every call it fails (`GitError` holds an `io::Error`, so it cannot clone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeFailure {
    NotFound,
    Spawn(std::io::ErrorKind),
    Exit { code: Option<i32>, stderr: String },
    Utf8,
    Parse(String),
    Timeout(Duration),
}

impl FakeFailure {
    pub fn to_error(&self) -> GitError {
        match self {
            Self::NotFound => GitError::NotFound,
            Self::Spawn(kind) => GitError::Spawn(std::io::Error::from(*kind)),
            Self::Exit { code, stderr } => GitError::Exit {
                code: *code,
                stderr: stderr.clone(),
            },
            Self::Utf8 => GitError::Utf8,
            Self::Parse(text) => GitError::Parse(text.clone()),
            Self::Timeout(after) => GitError::Timeout(*after),
        }
    }
}

/// What real git says outside a repository.
fn not_a_repo() -> GitError {
    fatal("not a git repository (or any of the parent directories): .git".to_owned())
}

/// Git's fatal-error exit, the class every "nothing there" answer uses.
fn fatal(message: String) -> GitError {
    GitError::Exit {
        code: Some(128),
        stderr: format!("fatal: {message}"),
    }
}

/// A scripted [`Git`]. Clones share one state, so a test keeps a clone to
/// configure and inspect while the code under test holds `Arc<dyn Git>`.
///
/// A handle resolves the registered repository whose root contains its path
/// (the deepest root wins); with none it answers as a plain directory, the
/// way real git answers outside a repository. A test that wants "after the
/// commit the tree is clean" says so with [`FakeGit::set_status`].
///
/// | Method | Plain directory | Registered, field unset |
/// |---|---|---|
/// | `available` | `true` | `true` |
/// | `probe` | `None` | `Some`; lanes = one main entry at the root |
/// | `tracking` | exit 128 | `branch` from [`FakeRepo::branch`], no upstream |
/// | `worktree_status` | exit 128 | clean (`Default`) |
/// | `status_scoped`, `diff`, `diff_untracked`, `diff_range` | exit 128 | `""` |
/// | `show_staged`, `show_at`, `head_message` | exit 128 | exit 128 (nothing there) |
/// | `default_branch` | `None` | [`FakeRepo::branch`] |
/// | `dirs` | exit 128 | `<root>/.git` for both |
/// | `push_target` | exit 128 | `None` (no remote) |
/// | `base_tips` | exit 128 as `BaseProblem::Git` | `NotFound(name)` |
/// | `changes_since` | exit 128 as `BaseProblem::Git` | no files, no commits |
/// | `init` | logged, `Ok` (registers nothing) | logged, `Ok` |
/// | other writes | logged, then exit 128 | logged, `Ok` |
/// | `merge` | logged, then exit 128 | logged, `Success` |
///
/// `base_tips` applies the real one's rules in its order: no base name is
/// always `NoBaseConfigured`, then a scripted base the current branch names is
/// `OnBaseBranch`, even when the script is `UnbornHead` (HEAD is read last).
/// Git-level failures of a base read come from [`FakeGit::fail`]; other
/// [`BaseProblem`](super::base::BaseProblem)s are scripted through [`FakeRepo::base_tips`].
///
/// Failures and call counts are keyed by the exact path a handle was opened
/// on (or `init`/`probe` was given), not the repository it resolves to.
/// `available` is counted under the empty path.
#[derive(Debug, Clone)]
pub struct FakeGit {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Debug)]
struct FakeState {
    available: bool,
    repos: Vec<FakeRepo>,
    calls: Vec<GitCall>,
    counts: HashMap<(GitOp, PathBuf), usize>,
    failures: HashMap<(PathBuf, GitOp), FakeFailure>,
}

impl Default for FakeGit {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeGit {
    /// Git installed, no repositories registered.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeState {
                available: true,
                repos: Vec::new(),
                calls: Vec::new(),
                counts: HashMap::new(),
                failures: HashMap::new(),
            })),
        }
    }

    /// Builder form of [`FakeGit::add_repo`].
    pub fn with(self, repo: FakeRepo) -> Self {
        self.add_repo(repo);
        self
    }

    /// Register `repo`, replacing one already registered at the same root.
    pub fn add_repo(&self, repo: FakeRepo) {
        let mut state = self.lock();
        state.repos.retain(|r| r.root() != repo.root());
        state.repos.push(repo);
    }

    /// Model git being installed or missing. Missing git fails every command
    /// with [`GitError::NotFound`] and probes nothing. The real
    /// `diff_untracked`, `show_staged`, and `show_at` skip the `PATH` check
    /// and fail with `GitError::Spawn(NotFound)` instead.
    pub fn set_available(&self, available: bool) {
        self.lock().available = available;
    }

    /// Fail every later `op` on a handle opened at exactly `path`. `Open` and
    /// `Available` cannot fail, so a failure aimed at them is ignored; one on
    /// `default_branch` or `probe` makes it answer `None`.
    pub fn fail(&self, path: impl Into<PathBuf>, op: GitOp, failure: FakeFailure) {
        self.lock().failures.insert((path.into(), op), failure);
    }

    /// Every write so far, in call order.
    pub fn calls(&self) -> Vec<GitCall> {
        self.lock().calls.clone()
    }

    /// How many times `op` was called on a handle opened at exactly `path`.
    /// Every call counts, writes and failed calls included.
    pub fn reads(&self, op: GitOp, path: &Path) -> usize {
        self.lock()
            .counts
            .get(&(op, path.to_path_buf()))
            .copied()
            .unwrap_or(0)
    }

    pub fn set_status(&self, root: &Path, status: GitWorktreeStatus) {
        self.edit_repo(root, |repo| repo.status = Some(status));
    }

    pub fn set_tracking(&self, root: &Path, tracking: GitTracking) {
        self.edit_repo(root, |repo| repo.tracking = Some(tracking));
    }

    pub fn set_worktrees(&self, root: &Path, worktrees: Vec<GitWorktreeInfo>) {
        self.edit_repo(root, |repo| repo.worktrees = Some(worktrees));
    }

    /// Edit the repository registered at exactly `root`. An unregistered
    /// root is a mistake in the test, so it panics rather than doing nothing.
    fn edit_repo(&self, root: &Path, edit: impl FnOnce(&mut FakeRepo)) {
        let mut state = self.lock();
        let Some(repo) = state.repos.iter_mut().find(|r| r.root() == root) else {
            panic!("FakeGit: no repository registered at {}", root.display());
        };
        edit(repo);
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        lock(&self.state)
    }
}

/// A test that panicked mid-call must not turn every later read into a
/// second, unrelated poison panic.
fn lock(state: &Mutex<FakeState>) -> MutexGuard<'_, FakeState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeState {
    fn count(&mut self, op: GitOp, path: &Path) {
        *self.counts.entry((op, path.to_path_buf())).or_default() += 1;
    }

    /// Missing git, then an injected failure, in the order a real call meets them.
    fn gate(&self, op: GitOp, path: &Path) -> Result<(), GitError> {
        if !self.available {
            return Err(GitError::NotFound);
        }
        match self.failures.get(&(path.to_path_buf(), op)) {
            Some(failure) => Err(failure.to_error()),
            None => Ok(()),
        }
    }

    /// The registered repository whose root holds `path`; the deepest wins so
    /// a lane nested under its main worktree answers for itself.
    fn repo_for(&self, path: &Path) -> Option<&FakeRepo> {
        self.repos
            .iter()
            .filter(|repo| daruda_core::path::is_within(path, repo.root()))
            .max_by_key(|repo| repo.root().components().count())
    }
}

impl Git for FakeGit {
    fn available(&self) -> bool {
        let mut state = self.lock();
        state.count(GitOp::Available, Path::new(""));
        state.available
    }

    fn init(&self, path: &Path) -> Result<(), GitError> {
        let mut state = self.lock();
        state.count(GitOp::Init, path);
        state.calls.push(GitCall::Init {
            path: path.to_path_buf(),
        });
        state.gate(GitOp::Init, path)
    }

    fn probe(&self, project_root: &Path) -> Option<RepoProbe> {
        let mut state = self.lock();
        state.count(GitOp::Probe, project_root);
        state.gate(GitOp::Probe, project_root).ok()?;
        state.repo_for(project_root).map(FakeRepo::probe)
    }

    fn open(&self, worktree: &Path) -> Arc<dyn GitRepo> {
        self.lock().count(GitOp::Open, worktree);
        Arc::new(handle::FakeHandle::new(
            worktree.to_path_buf(),
            Arc::clone(&self.state),
        ))
    }
}

#[cfg(test)]
mod tests;
