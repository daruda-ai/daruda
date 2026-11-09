//! Real-git fixtures for the git tests: setup runs a plain `git` child under
//! the isolated test config, and the operations under test go through
//! [`SystemGit`], the only public way into the CLI.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use daruda_project::lane::git::{Git, GitRepo, GitWorktreeInfo, SystemGit, isolated_git_env};

pub fn has_git() -> bool {
    SystemGit.available()
}

pub fn open(worktree: &Path) -> Arc<dyn GitRepo> {
    SystemGit.open(worktree)
}

/// `git <args>` in `dir`, asserting success; returns trimmed stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = daruda_core::process::command("git")
        .envs(isolated_git_env())
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} in {dir:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A fresh repository at `path`, creating the directory if missing.
pub fn init(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "--quiet"]);
}

/// Every worktree of the repository `path` belongs to.
pub fn lanes(path: &Path) -> Vec<GitWorktreeInfo> {
    SystemGit.probe(path).expect("a repository").lanes
}

/// The repository root `path` belongs to, if any.
pub fn repo_root(path: &Path) -> Option<PathBuf> {
    SystemGit.probe(path).map(|probe| probe.repo_root)
}
