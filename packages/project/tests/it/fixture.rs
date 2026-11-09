//! Real-git fixtures for the git tests: setup runs a plain `git` child under
//! the isolated test config, and the operations under test go through
//! [`SystemGit`], the only public way into the CLI. Every test reaches git
//! through these helpers, so none can run before isolation is on.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use daruda_project::lane::git::{
    Git, GitRepo, GitWorktreeInfo, SystemGit, isolate_git_config_for_tests,
};

/// The real git backend, with the developer's own config swapped out first.
pub fn system_git() -> SystemGit {
    isolate_git_config_for_tests();
    SystemGit
}

pub fn has_git() -> bool {
    system_git().available()
}

pub fn open(worktree: &Path) -> Arc<dyn GitRepo> {
    system_git().open(worktree)
}

/// `git <args>` in `dir` under the isolated config, whatever its exit status.
pub fn git_output(dir: &Path, args: &[&str]) -> Output {
    daruda_core::process::command("git")
        .envs(isolate_git_config_for_tests())
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

/// `git <args>` in `dir`, asserting success; returns trimmed stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_output(dir, args);
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
    system_git().probe(path).expect("a repository").lanes
}

/// The repository root `path` belongs to, if any.
pub fn repo_root(path: &Path) -> Option<PathBuf> {
    system_git().probe(path).map(|probe| probe.repo_root)
}
