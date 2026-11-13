//! Blocking wrappers around the `git` CLI for lane operations.
//!
//! Synchronous on purpose — the UI layer invokes these from
//! `cx.background_spawn` so the main thread stays free. Keeping this
//! module GPUI-free means it's unit-testable with a temp-directory
//! fixture and also reusable from CLI tools later.
//!
//! Every function shells out to `git`. If `git` isn't on `PATH` the
//! wrappers return `GitError::NotFound`; callers decide whether to fall
//! back (e.g. treat as "not a repo") or surface the error in the
//! status bar.
//!
//! The status / diff / staging / commit / push family lives in
//! `status`; this module retains the run-git plumbing, the repo
//! probe, lane lifecycle, and the merge sub-module. [`base`] answers what a
//! lane committed since the branch it left.

mod backend;
pub mod base;
mod command;
mod environment;
pub use environment::GitEnvironmentFailure;
pub mod discard;
mod remote;
mod removal;
pub use remote::PushTarget;
pub use removal::{LaneRemovalState, RemovalError};
mod status;

pub use backend::{Git, GitRepo, SystemGit};
pub(crate) use command::git_command;
#[cfg(any(test, feature = "test-support"))]
pub use command::isolate_git_config_for_tests;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
#[cfg(any(test, feature = "test-support"))]
pub use fake::{FakeFailure, FakeGit, FakeRepo, GitCall, GitOp};
pub(crate) use status::*;
pub use status::{GitFileEntry, GitTracking, GitWorktreeStatus};

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Lane entry parsed from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitWorktreeInfo {
    pub path: PathBuf,
    /// `None` = detached HEAD (no attached branch).
    pub branch: Option<String>,
    pub head: Option<String>,
    /// True for the "bare" marker — daruda usually filters these out.
    pub bare: bool,
}

#[derive(Debug)]
pub enum GitError {
    /// `git` is not installed or not on `PATH` (pre-spawn `which` probe).
    NotFound,
    /// Couldn't launch `git` at all (not installed, permission, etc.).
    Spawn(std::io::Error),
    /// `git` ran but exited non-zero.
    Exit { code: Option<i32>, stderr: String },
    /// Output wasn't valid UTF-8.
    Utf8,
    /// `git worktree list --porcelain` returned an unparseable chunk.
    Parse(String),
    /// `git` was killed because it ran past the configured deadline. The
    /// usual cause is a network operation (`push`, `fetch`, `pull`) on a
    /// stalled connection. Surfaces in the status bar so the user knows
    /// the lane isn't actually frozen.
    Timeout(Duration),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::NotFound => write!(f, "git is not installed or not on PATH"),
            GitError::Spawn(e) => write!(f, "spawn git: {e}"),
            GitError::Exit {
                code: Some(c),
                stderr,
            } => write!(f, "git exited {c}: {}", stderr.trim()),
            GitError::Exit { code: None, stderr } => {
                write!(f, "git terminated by signal: {}", stderr.trim())
            }
            GitError::Utf8 => write!(f, "git output not valid UTF-8"),
            GitError::Parse(s) => write!(f, "unexpected git porcelain output: {s}"),
            GitError::Timeout(d) => write!(f, "git timed out after {}s", d.as_secs()),
        }
    }
}

/// Default deadline for every `run_git` invocation. Network operations
/// (`push`, `fetch`, `pull`) on a slow connection can legitimately take
/// 30s+; 60s gives margin while still detecting a stalled connection
/// before the user assumes the UI is frozen. Local operations
/// (`status`, `log`, `rev-parse`) complete in well under a second so the
/// poll overhead is invisible.
const RUN_GIT_TIMEOUT: Duration = Duration::from_secs(60);

impl std::error::Error for GitError {}

/// Run `git <args>` in `cwd` with a `RUN_GIT_TIMEOUT` deadline. Returns
/// stdout as a String on success. If the deadline elapses, git and the
/// helpers it forked are killed and the call returns `GitError::Timeout` —
/// callers must treat this distinctly from a clean failure (e.g. the
/// commit/push state is unknown when this fires).
pub(super) fn run_git<I, S>(cwd: &Path, args: I) -> Result<String, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_git_with_timeout(cwd, args, RUN_GIT_TIMEOUT)
}

/// Like [`run_git`] but with an explicit timeout. Exposed for tests so
/// they can pin a short deadline without waiting `RUN_GIT_TIMEOUT`.
fn run_git_with_timeout<I, S>(cwd: &Path, args: I, timeout: Duration) -> Result<String, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    // Pre-spawn guard: surface a clear "git not on PATH" error instead
    // of an opaque spawn failure when git isn't installed.
    which::which("git").map_err(|_| GitError::NotFound)?;

    let mut command = git_command(cwd);
    command.args(args);
    // git forks helpers of its own (ssh, askpass, credential); the runner
    // ends them together with git when a bound is crossed.
    let limits = daruda_core::process::CaptureLimits {
        timeout,
        stdout: 64 * 1024 * 1024,
        stderr: 1024 * 1024,
    };
    let out = daruda_core::process::run_captured(command, limits).map_err(|e| match e {
        daruda_core::process::CaptureError::Spawn(e) => GitError::Spawn(e),
        daruda_core::process::CaptureError::Timeout(d) => GitError::Timeout(d),
        daruda_core::process::CaptureError::Overflow { limit } => {
            GitError::Parse(format!("git output exceeded {limit} bytes"))
        }
    })?;
    if !out.status.success() {
        return Err(GitError::Exit {
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    String::from_utf8(out.stdout).map_err(|_| GitError::Utf8)
}

/// `git init` in `path`. Used by the Git Changes view's "Initialize
/// Git Repo" button — turns a non-git worktree into a fresh repo so
/// the user can start staging immediately.
pub(crate) fn git_init(path: &Path) -> Result<(), GitError> {
    run_git(path, ["init"]).map(|_| ())
}

/// Cheap availability probe — runs `git --version`. Use this to
/// decide whether to show the "Initialize Git Repo" button vs. hide
/// the lane UI entirely.
///
/// A success is remembered, so repeated callers cost no process; a failure
/// is not, so installing git while daruda runs is still picked up.
pub(crate) fn has_git() -> bool {
    static FOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if FOUND.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    let found = daruda_core::process::command("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if found {
        FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    found
}

/// True when `path` is inside a git repository (any lane counts).
pub(crate) fn is_git_repo(path: &Path) -> bool {
    run_git(path, ["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false)
}

/// Top-level directory of the repo that owns `path` (the common
/// git dir). Returns `None` when `path` isn't inside a repo.
pub(crate) fn repo_root(path: &Path) -> Option<PathBuf> {
    run_git(path, ["rev-parse", "--show-toplevel"])
        .ok()
        .map(|s| PathBuf::from(s.trim()))
}

/// Currently-checked-out branch name at `path`, or `None` for detached
/// HEAD / non-repo. Uses `git symbolic-ref --short HEAD` which fails
/// cleanly when detached — we map that to `Ok(None)` rather than an
/// error so the caller can render "(detached)" without a special case.
pub(crate) fn current_branch(path: &Path) -> Result<Option<String>, GitError> {
    match run_git(path, ["symbolic-ref", "--short", "HEAD"]) {
        Ok(s) => Ok(Some(s.trim().to_string())),
        Err(GitError::Exit { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Repo's default branch, best-effort. None on non-repo / detached.
///
/// Tries `origin/HEAD` first (the remote's published default), then
/// the conventional local `main` / `master`, and finally falls back
/// to whatever branch is checked out. Detection lives here because
/// `daruda_store` is git-free and only holds the persisted value.
pub(crate) fn default_branch(repo_root: &Path) -> Option<String> {
    if let Ok(s) = run_git(
        repo_root,
        ["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) && let Some(b) = s.trim().strip_prefix("origin/")
    {
        return Some(b.to_string());
    }
    for cand in ["main", "master"] {
        if run_git(
            repo_root,
            [
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/heads/{cand}"),
            ],
        )
        .is_ok()
        {
            return Some(cand.into());
        }
    }
    current_branch(repo_root).ok().flatten()
}

/// `git worktree add <new_path> [-b <new_branch>] [<base>]`.
///
/// * `new_branch = Some(_)` → create a new branch (`-b`) starting at
///   `base` (e.g. `"main"`, `"origin/main"`, a SHA), or at HEAD for `None`.
/// * `new_branch = None`, `base = Some(branch)` → check out that existing
///   local branch.
/// * Both `None` → git's default: the branch named after `new_path`'s
///   basename.
///
/// `repo_root` must be the main repository (any existing lane
/// path works; git resolves the shared gitdir).
pub(crate) fn add_lane(
    repo_root: &Path,
    new_path: &Path,
    new_branch: Option<&str>,
    base: Option<&str>,
) -> Result<(), GitError> {
    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];
    if let Some(b) = new_branch {
        args.push("-b".into());
        args.push(b.to_string());
    }
    args.push(new_path.to_string_lossy().into_owned());
    match (new_branch, base) {
        // Force commit resolution so `origin/<base>`-style refs do
        // not become the new branch's upstream — without `^{commit}`
        // git auto-tracks the remote ref, which then triggers
        // unintended `push` collisions when daruda later finalizes
        // the lane. Mirrors superset-desktop git.ts:457-519.
        (Some(_), Some(b)) => args.push(format!("{b}^{{commit}}")),
        // Bare, or git would detach HEAD at the commit instead of checking
        // the branch out.
        (None, Some(b)) => args.push(b.to_owned()),
        (_, None) => {}
    }
    run_git(repo_root, args)?;

    // `push.autoSetupRemote=true` makes the *first* `git push` from
    // the new lane auto-create the remote branch and set its
    // upstream — the user never has to type `-u origin <branch>`.
    // Failure here is non-fatal: the lane is already on disk.
    let _ = run_git(
        new_path,
        ["config", "--local", "push.autoSetupRemote", "true"],
    );

    Ok(())
}

/// Remove a linked worktree; Git rejects removing the main checkout.
/// A missing checkout is success only after a successful listing confirms
/// its registration is gone too. An unusable repository must still fail.
pub(crate) fn remove_lane(repo_root: &Path, path: &Path, force: bool) -> Result<(), GitError> {
    let mut args: Vec<String> = vec!["worktree".into(), "remove".into()];
    if force {
        args.push("--force".into());
    }
    args.push(path.to_string_lossy().into_owned());
    match run_git(repo_root, args) {
        Ok(_) => Ok(()),
        Err(error @ GitError::Exit { code, .. }) if already_removed(code, path) => {
            if removal::registered(repo_root, path)? {
                Err(error)
            } else {
                Ok(())
            }
        }
        Err(e) => Err(e),
    }
}

/// Recognize a candidate already-removed checkout without matching stderr.
/// The caller must also verify that Git no longer lists its registration.
/// Permission errors and dangling symlinks do not prove a path is missing.
fn already_removed(code: Option<i32>, path: &Path) -> bool {
    code == Some(128)
        && std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// `git branch -D <branch>` — force-delete a branch. Used by the
/// "remove lane + branch" flow: after `lane remove` succeeds,
/// the branch is no longer checked out anywhere, so deletion is safe
/// (git still rejects deleting the currently-checked-out branch of
/// the main lane, surfacing as `GitError::Exit`).
pub(crate) fn delete_branch(repo_root: &Path, branch: &str) -> Result<(), GitError> {
    run_git(repo_root, ["branch", "-D", branch]).map(|_| ())
}

/// `git worktree list --porcelain -z`. Returns one entry per linked
/// lane (including the main one). The porcelain format is
/// stable: NUL-separated fields, with an empty field between records.
/// Unknown keys are ignored so future git versions don't break us.
pub(crate) fn list_worktrees(repo_root: &Path) -> Result<Vec<GitWorktreeInfo>, GitError> {
    let stdout = run_git(repo_root, ["worktree", "list", "--porcelain", "-z"])?;
    parse_worktree_list(&stdout)
}

/// Absolute paths to the two git directories a lane's state lives in.
///
/// They differ for a linked worktree: `index` and `HEAD` are per-worktree,
/// while `refs/` and `packed-refs` are shared, so a fetch in one lane moves
/// state every lane of the repo reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// This lane's own git dir — `<main>/.git/worktrees/<name>` for a
    /// linked worktree, `<root>/.git` for the main one.
    pub git_dir: PathBuf,
    /// The git dir every lane of the repository shares.
    pub common_dir: PathBuf,
}

/// `git rev-parse --git-dir --git-common-dir` — both paths in one call.
/// Git answers with paths relative to `path` for the main worktree, so
/// they are resolved here and the caller always gets absolute ones.
pub(crate) fn git_dirs(path: &Path) -> Result<GitDirs, GitError> {
    let out = run_git(path, ["rev-parse", "--git-dir", "--git-common-dir"])?;
    let mut lines = out.lines();
    let (Some(git_dir), Some(common_dir)) = (lines.next(), lines.next()) else {
        return Err(GitError::Parse(out));
    };
    let canonical = |p: &str| {
        let p = Path::new(p.trim());
        let absolute = if p.is_absolute() {
            p.to_path_buf()
        } else {
            path.join(p)
        };
        daruda_core::path::canonicalize_or_self(absolute)
    };
    Ok(GitDirs {
        git_dir: canonical(git_dir),
        common_dir: canonical(common_dir),
    })
}

/// Aggregated repo probe: combines `is_git_repo` + `repo_root` +
/// `list_worktrees` into one call. Returns `None` when git is unusable or
/// the path isn't a repo — a missing git already fails `is_git_repo` — so
/// callers can branch on a single Option instead of threading the probes.
pub struct RepoProbe {
    pub repo_root: PathBuf,
    pub lanes: Vec<GitWorktreeInfo>,
}

pub(crate) fn probe_repo(project_root: &Path) -> Option<RepoProbe> {
    if !is_git_repo(project_root) {
        return None;
    }
    let repo_root = repo_root(project_root)?;
    let lanes = list_worktrees(&repo_root).ok()?;
    Some(RepoProbe { repo_root, lanes })
}

fn parse_worktree_list(text: &str) -> Result<Vec<GitWorktreeInfo>, GitError> {
    let mut out = Vec::new();
    let mut cur: Option<GitWorktreeInfo> = None;

    let flush = |cur: &mut Option<GitWorktreeInfo>, out: &mut Vec<GitWorktreeInfo>| {
        if let Some(wt) = cur.take() {
            out.push(wt);
        }
    };

    // NUL records preserve newlines and trailing spaces inside paths.
    for line in text.split('\0') {
        if line.is_empty() {
            flush(&mut cur, &mut out);
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            flush(&mut cur, &mut out);
            cur = Some(GitWorktreeInfo {
                path: PathBuf::from(rest),
                branch: None,
                head: None,
                bare: false,
            });
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            let Some(wt) = cur.as_mut() else {
                return Err(GitError::Parse(line.to_string()));
            };
            wt.head = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            let Some(wt) = cur.as_mut() else {
                return Err(GitError::Parse(line.to_string()));
            };
            // `branch refs/heads/main` → `main`.
            wt.branch = Some(rest.strip_prefix("refs/heads/").unwrap_or(rest).to_string());
        } else if line == "bare" {
            let Some(wt) = cur.as_mut() else {
                return Err(GitError::Parse(line.to_string()));
            };
            wt.bare = true;
        } else if line == "detached" {
            // Explicit detached marker — leave `branch: None`.
            if cur.is_none() {
                return Err(GitError::Parse(line.to_string()));
            }
        }
        // Silently skip unknown keys (locked, prunable, …) — we only
        // need path/branch/HEAD for the left dock.
    }
    flush(&mut cur, &mut out);
    Ok(out)
}

// ----------------------------------------------------------------
// Git status + diff + commit
// ----------------------------------------------------------------
/// Outcome of a `git merge` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// `git merge` reported "Already up to date."
    AlreadyUpToDate,
    /// Merge completed (fast-forward or new merge commit).
    Success,
    /// One or more files have conflicts.  The target lane is left in
    /// mid-merge state so the user can resolve conflicts manually and commit.
    /// The caller may call `git_merge_abort` to cancel instead.
    Conflicts(Vec<String>),
}

/// `git merge --no-ff --no-edit <source_branch>` run from `target_path`.
///
/// Executed in the *target* lane directory so the merge lands on
/// the branch that is already checked out there — no `checkout` needed.
/// `--no-ff` always creates a merge commit, preserving branch history even
/// when a fast-forward would be possible.
///
/// Exit 0 + "Already up to date." stdout → `AlreadyUpToDate`.
/// Exit 0 otherwise → `Success`.
/// Exit 1 → the unmerged paths, as git lists them → `Conflicts(files)`;
/// none unmerged → `Err`, since git declined to merge at all.
/// Any other exit code → `Err(GitError::Exit)`.
pub(crate) fn git_merge(target_path: &Path, source_branch: &str) -> Result<MergeOutcome, GitError> {
    let output = git_command(target_path)
        .args(["merge", "--no-ff", "--no-edit", source_branch])
        .output()
        .map_err(GitError::Spawn)?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

    match output.status.code() {
        Some(0) => {
            if stdout.trim() == "Already up to date." {
                Ok(MergeOutcome::AlreadyUpToDate)
            } else {
                Ok(MergeOutcome::Success)
            }
        }
        Some(1) => {
            // Ask git which paths are unmerged rather than reading them out
            // of its prose: a `CONFLICT` line spells the path inside a
            // sentence, so a name with a space was cut at the first word.
            // Plumbing: porcelain `git diff` would also rewrite the index.
            let unmerged = run_git(
                target_path,
                ["diff-files", "--name-only", "--diff-filter=U", "-z"],
            )?;
            let files: Vec<String> = unmerged
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect();
            // Exit 1 with nothing unmerged leaves nothing to resolve, so it
            // is reported as the failure it is rather than an empty conflict.
            if files.is_empty() {
                return Err(GitError::Exit {
                    code: output.status.code(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            Ok(MergeOutcome::Conflicts(files))
        }
        _ => Err(GitError::Exit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
    }
}

/// `git merge --abort` — cancel an in-progress conflicted merge and
/// restore the target lane to its pre-merge state.
pub(crate) fn git_merge_abort(target_path: &Path) -> Result<(), GitError> {
    run_git(target_path, ["merge", "--abort"]).map(|_| ())
}

// ----------------------------------------------------------------
// Tests
// ----------------------------------------------------------------

#[cfg(test)]
mod tests;
